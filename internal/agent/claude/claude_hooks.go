package claude

import (
	"encoding/json"
	"fmt"
	"github.com/pomelohq/pomelo/internal/httpx"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/pomelohq/pomelo/internal/paths"
	"github.com/pomelohq/pomelo/internal/pombin"
	"github.com/pomelohq/pomelo/internal/services"
)

func wsStateKey(branch string, isMain bool) string {
	if isMain {
		return "main:" + branch
	}
	return "ws:" + branch
}

func hookEventState(event, notifyType string) (string, bool) {
	switch event {
	case "SessionStart":
		return "idle", true
	case "UserPromptSubmit":
		return "thinking", true
	case "PreToolUse", "PostToolUse", "PostToolBatch":
		return "tool_use", true
	case "PreCompact":
		return "compacting", true
	case "Notification":
		// Only a permission/elicitation prompt genuinely blocks on the user.
		// Claude's idle-timeout notification is "Claude is waiting for your input"
		// — matching the bare word "input" lit every idle agent red. Anything
		// that is not a permission/elicitation prompt is just idle.
		if strings.Contains(notifyType, "permission") || strings.Contains(notifyType, "elicitation") {
			return "awaiting_input", true
		}
		return "idle", true
	case "Stop", "SessionEnd":
		return "idle", true
	}
	return "", false
}

func (s *Feature) handleClaudeHook(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpx.Err(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	branch := r.URL.Query().Get("branch")
	if branch == "" {
		httpx.Err(w, http.StatusBadRequest, "missing branch")
		return
	}
	isMain := r.URL.Query().Get("is_main") == "true"
	svc := r.URL.Query().Get("svc")
	if svc == "" {
		svc = s.claudeServiceName()
	}

	var body struct {
		Event      string `json:"hook_event_name"`
		NotifyType string `json:"notification_type"`
	}
	_ = json.NewDecoder(r.Body).Decode(&body)

	state, ok := hookEventState(body.Event, body.NotifyType)
	if !ok {
		httpx.Write(w, map[string]any{"ok": true, "ignored": body.Event})
		return
	}

	key := wsStateKey(branch, isMain)
	prev := s.getAgentState(key)
	if prev == "" {
		prev = "idle"
	}
	if prev == state {
		httpx.Write(w, map[string]any{"ok": true, "state": state})
		return
	}
	s.broadcastAgent(agentEvent{Branch: branch, IsMain: isMain, Service: svc, State: state, Prev: prev})
	httpx.Write(w, map[string]any{"ok": true, "state": state})
}

var claudeHookEvents = []string{
	"SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse",
	"PreCompact", "Stop", "SessionEnd", "Notification",
}

func WriteHookState(rawStdin []byte) error {
	var body struct {
		Cwd        string `json:"cwd"`
		Event      string `json:"hook_event_name"`
		NotifyType string `json:"notification_type"`
	}
	if json.Unmarshal(rawStdin, &body) != nil {
		return nil
	}
	branch := branchFromCwd(body.Cwd)
	if branch == "" {
		return nil
	}
	notify := body.NotifyType
	if body.Event == "Notification" {
		low := strings.ToLower(string(rawStdin))
		switch {
		case strings.Contains(low, "permission") || strings.Contains(low, "elicitation"):
			notify = "permission"
		default:
			notify = "idle"
		}
	}
	state, ok := hookEventState(body.Event, notify)
	if !ok {
		return nil
	}

	dir := paths.StatePath("agents")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("mkdir agents state: %w", err)
	}
	path := filepath.Join(dir, "state-"+services.BranchSafe(branch)+".json")
	blob := fmt.Sprintf(`{"branch":%s,"state":%q}`, strconv.Quote(branch), state)
	if err := os.WriteFile(path, []byte(blob), 0o644); err != nil {
		return fmt.Errorf("write agent state %s: %w", path, err)
	}
	return nil
}

func branchFromCwd(cwd string) string {
	for _, seg := range strings.Split(filepath.Clean(cwd), string(filepath.Separator)) {
		if b, ok := strings.CutPrefix(seg, "workspace--"); ok && b != "" {
			return b
		}
	}
	return ""
}

func InstallGlobalClaudeHook() error {
	home, err := os.UserHomeDir()
	if err != nil {
		return fmt.Errorf("home dir: %w", err)
	}
	bin, err := pombin.Path()
	if err != nil || bin == "" {
		bin = "pom"
	}
	stateDir := filepath.Join(home, ".local", "state", "pom")
	if err := os.MkdirAll(stateDir, 0o755); err != nil {
		return fmt.Errorf("mkdir state: %w", err)
	}
	script := filepath.Join(stateDir, "claude-hook")
	body := "#!/bin/sh\nBIN='" + strings.ReplaceAll(bin, "'", `'\''`) + "'\n[ -x \"$BIN\" ] || exit 0\nexec \"$BIN\" claude-hook\n"
	if err := os.WriteFile(script, []byte(body), 0o755); err != nil {
		return fmt.Errorf("write hook script: %w", err)
	}
	_ = os.Remove(filepath.Join(stateDir, "pom-hook-bin"))
	command := "sh " + script

	dir := filepath.Join(home, ".claude")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("mkdir .claude: %w", err)
	}
	path := filepath.Join(dir, "settings.json")
	root := map[string]any{}
	if b, err := os.ReadFile(path); err == nil && len(b) > 0 {
		if json.Unmarshal(b, &root) != nil {
			root = map[string]any{}
		}
	}
	hooks, _ := root["hooks"].(map[string]any)
	if hooks == nil {
		hooks = map[string]any{}
	}
	ourEntry := map[string]any{
		"matcher": "",
		"hooks":   []any{map[string]any{"type": "command", "command": command}},
	}
	for _, ev := range claudeHookEvents {
		list, _ := hooks[ev].([]any)
		list = pruneHookCommands(list, "claude-hook")
		hooks[ev] = append(list, ourEntry)
	}
	root["hooks"] = hooks

	blob, err := json.MarshalIndent(root, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal settings: %w", err)
	}
	if err := os.WriteFile(path, blob, 0o644); err != nil {
		return fmt.Errorf("write %s: %w", path, err)
	}
	return nil
}

func InstallGlobalClaudeMCP() error {
	home, err := os.UserHomeDir()
	if err != nil {
		return fmt.Errorf("home dir: %w", err)
	}
	bin, err := pombin.Path()
	if err != nil || bin == "" {
		bin = "pom"
	}
	stateDir := filepath.Join(home, ".local", "state", "pom")
	if err := os.MkdirAll(stateDir, 0o755); err != nil {
		return fmt.Errorf("mkdir state: %w", err)
	}
	script := filepath.Join(stateDir, "pom-mcp")
	body := "#!/bin/sh\nBIN='" + strings.ReplaceAll(bin, "'", `'\''`) + "'\n[ -x \"$BIN\" ] || exit 0\nexec \"$BIN\" mcp\n"
	if err := os.WriteFile(script, []byte(body), 0o755); err != nil {
		return fmt.Errorf("write mcp wrapper: %w", err)
	}
	_ = os.Remove(filepath.Join(stateDir, "pom-mcp-bin"))

	path := filepath.Join(home, ".claude.json")
	root := map[string]any{}
	if b, err := os.ReadFile(path); err == nil && len(b) > 0 {
		if json.Unmarshal(b, &root) != nil {
			return fmt.Errorf("~/.claude.json is not valid JSON — not modifying it")
		}
	}
	servers, _ := root["mcpServers"].(map[string]any)
	if servers == nil {
		servers = map[string]any{}
	}
	servers["pom"] = map[string]any{"command": "sh", "args": []any{script}, "env": map[string]any{}}
	root["mcpServers"] = servers
	blob, err := json.MarshalIndent(root, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal ~/.claude.json: %w", err)
	}
	if err := os.WriteFile(path, blob, 0o644); err != nil {
		return fmt.Errorf("write %s: %w", path, err)
	}
	return nil
}

func pruneHookCommands(list []any, needle string) []any {
	out := list[:0:0]
	for _, e := range list {
		m, ok := e.(map[string]any)
		if !ok {
			out = append(out, e)
			continue
		}
		inner, _ := m["hooks"].([]any)
		kept := inner[:0:0]
		for _, h := range inner {
			hm, ok := h.(map[string]any)
			if ok {
				if cmd, _ := hm["command"].(string); strings.Contains(cmd, needle) {
					continue
				}
			}
			kept = append(kept, h)
		}
		if len(kept) == 0 {
			continue
		}
		m["hooks"] = kept
		out = append(out, m)
	}
	return out
}
