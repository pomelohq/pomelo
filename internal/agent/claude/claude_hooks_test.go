package claude

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/pomelohq/pomelo/internal/paths"
	"github.com/pomelohq/pomelo/internal/services"
)

func TestBranchFromCwd(t *testing.T) {
	cases := map[string]string{
		"/Users/x/Workspaces/proj/workspace--feat-login/api": "feat-login",
		"/Users/x/Workspaces/proj/workspace--feat-login":     "feat-login",
		"/Users/x/Workspaces/proj/workspace--main/api/sub":   "main",
		"/Users/x/Workspaces/proj/api":                       "",
		"/Users/x/workspace--/api":                           "",
	}
	for in, want := range cases {
		if got := branchFromCwd(in); got != want {
			t.Errorf("branchFromCwd(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestWriteHookState(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	cwd := "/x/proj/workspace--feat-x/api"
	in, _ := json.Marshal(map[string]any{"cwd": cwd, "hook_event_name": "UserPromptSubmit"})
	if err := WriteHookState(in); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(paths.StatePath("agents"), "state-"+services.BranchSafe("feat-x")+".json")
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("state file not written: %v", err)
	}
	var f struct{ Branch, State string }
	if err := json.Unmarshal(b, &f); err != nil {
		t.Fatal(err)
	}
	if f.Branch != "feat-x" || f.State != "thinking" {
		t.Errorf("got %+v, want {feat-x thinking}", f)
	}
}

func TestWriteHookStateNonWorkspaceNoop(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("XDG_STATE_HOME", dir)
	in, _ := json.Marshal(map[string]any{"cwd": "/x/proj/api", "hook_event_name": "UserPromptSubmit"})
	if err := WriteHookState(in); err != nil {
		t.Fatal(err)
	}
	if entries, _ := os.ReadDir(paths.StatePath("agents")); len(entries) != 0 {
		t.Errorf("expected no state files for non-workspace cwd, got %d", len(entries))
	}
}

func TestWriteHookStateNotificationPermission(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	cwd := "/x/proj/workspace--b/api"
	in, _ := json.Marshal(map[string]any{"cwd": cwd, "hook_event_name": "Notification", "message": "Claude needs your permission to run"})
	if err := WriteHookState(in); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(filepath.Join(paths.StatePath("agents"), "state-b.json"))
	var f struct{ State string }
	_ = json.Unmarshal(b, &f)
	if f.State != "awaiting_input" {
		t.Errorf("got state %q, want awaiting_input", f.State)
	}
}

// Claude's 60s idle notification reads "Claude is waiting for your input" —
// the substring "input" must NOT be mistaken for a real prompt, or every idle
// agent shows the red awaiting-input dot.
func TestWriteHookStateNotificationIdleWaiting(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	cwd := "/x/proj/workspace--c/api"
	in, _ := json.Marshal(map[string]any{"cwd": cwd, "hook_event_name": "Notification", "message": "Claude is waiting for your input"})
	if err := WriteHookState(in); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(filepath.Join(paths.StatePath("agents"), "state-c.json"))
	var f struct{ State string }
	_ = json.Unmarshal(b, &f)
	if f.State != "idle" {
		t.Errorf("got state %q, want idle (idle-timeout notification is not a prompt)", f.State)
	}
}

func TestInstallGlobalClaudeHookIdempotent(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)

	if err := InstallGlobalClaudeHook(); err != nil {
		t.Fatal(err)
	}
	if err := InstallGlobalClaudeHook(); err != nil {
		t.Fatal(err)
	}

	b, err := os.ReadFile(filepath.Join(home, ".claude", "settings.json"))
	if err != nil {
		t.Fatal(err)
	}
	var root map[string]any
	if err := json.Unmarshal(b, &root); err != nil {
		t.Fatal(err)
	}
	hooks, ok := root["hooks"].(map[string]any)
	if !ok {
		t.Fatal("no hooks block")
	}
	for _, ev := range claudeHookEvents {
		list, _ := hooks[ev].([]any)
		if len(list) != 1 {
			t.Errorf("event %s: got %d entries, want 1 (idempotent)", ev, len(list))
		}
	}
}

func TestInstallGlobalClaudeHookPreservesExisting(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	dir := filepath.Join(home, ".claude")
	_ = os.MkdirAll(dir, 0o755)
	existing := map[string]any{
		"theme": "dark",
		"hooks": map[string]any{
			"SessionStart": []any{
				map[string]any{"matcher": "", "hooks": []any{map[string]any{"type": "command", "command": "other-tool"}}},
			},
		},
	}
	blob, _ := json.MarshalIndent(existing, "", "  ")
	_ = os.WriteFile(filepath.Join(dir, "settings.json"), blob, 0o644)

	if err := InstallGlobalClaudeHook(); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(filepath.Join(dir, "settings.json"))
	var root map[string]any
	_ = json.Unmarshal(b, &root)
	if root["theme"] != "dark" {
		t.Error("theme key clobbered")
	}
	list, _ := root["hooks"].(map[string]any)["SessionStart"].([]any)
	if len(list) != 2 {
		t.Errorf("SessionStart: got %d entries, want 2 (existing + ours)", len(list))
	}
}
