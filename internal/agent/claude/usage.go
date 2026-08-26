package claude

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"time"
)

type UsageWindow struct {
	Pct      float64 `json:"pct"`
	ResetsAt int64   `json:"resets_at"`
}

type Usage struct {
	OK      bool        `json:"ok"`
	Error   string      `json:"error,omitempty"`
	Session UsageWindow `json:"session"`
	Weekly  UsageWindow `json:"weekly"`
}

// claudeToken reads the Claude Code OAuth access token, file first then the macOS
// keychain (same locations Claude Code itself uses).
func claudeToken() string {
	home, _ := os.UserHomeDir()
	if data, err := os.ReadFile(filepath.Join(home, ".claude", ".credentials.json")); err == nil {
		if t := parseToken(data); t != "" {
			return t
		}
	}
	if out, err := exec.Command("security", "find-generic-password", "-s", "Claude Code-credentials", "-w").Output(); err == nil {
		if t := parseToken(out); t != "" {
			return t
		}
	}
	return ""
}

func parseToken(data []byte) string {
	var c struct {
		ClaudeAiOauth struct {
			AccessToken string `json:"accessToken"`
		} `json:"claudeAiOauth"`
	}
	_ = json.Unmarshal(data, &c)
	return c.ClaudeAiOauth.AccessToken
}

type usageWin struct {
	UsedPercentage *float64        `json:"used_percentage"`
	Utilization    *float64        `json:"utilization"`
	Percent        *float64        `json:"percent"`
	ResetsAt       json.RawMessage `json:"resets_at"`
}

func (w *usageWin) mapped() UsageWindow {
	if w == nil {
		return UsageWindow{}
	}
	pct := 0.0
	for _, p := range []*float64{w.UsedPercentage, w.Utilization, w.Percent} {
		if p != nil {
			pct = *p
			break
		}
	}
	return UsageWindow{Pct: pct, ResetsAt: parseResets(w.ResetsAt)}
}

func parseResets(raw json.RawMessage) int64 {
	if len(raw) == 0 {
		return 0
	}
	var n float64
	if json.Unmarshal(raw, &n) == nil && n > 0 {
		if n > 1e12 {
			return int64(n / 1000)
		}
		return int64(n)
	}
	var s string
	if json.Unmarshal(raw, &s) == nil {
		if t, err := time.Parse(time.RFC3339, s); err == nil {
			return t.Unix()
		}
	}
	return 0
}

// FetchUsage reports Claude subscription usage (5h session + weekly windows) via the
// same OAuth usage endpoint Claude Code reads for its status line.
func FetchUsage() Usage {
	tok := claudeToken()
	if tok == "" {
		return Usage{Error: "no Claude credentials"}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "https://api.anthropic.com/api/oauth/usage", nil)
	if err != nil {
		return Usage{Error: err.Error()}
	}
	req.Header.Set("Authorization", "Bearer "+tok)
	req.Header.Set("anthropic-beta", "oauth-2025-04-20")
	req.Header.Set("User-Agent", "claude-code/2.1.0")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return Usage{Error: err.Error()}
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return Usage{Error: "usage http " + strconv.Itoa(resp.StatusCode)}
	}
	var body struct {
		FiveHour *usageWin `json:"five_hour"`
		SevenDay *usageWin `json:"seven_day"`
	}
	if json.NewDecoder(resp.Body).Decode(&body) != nil {
		return Usage{Error: "decode failed"}
	}
	return Usage{OK: true, Session: body.FiveHour.mapped(), Weekly: body.SevenDay.mapped()}
}
