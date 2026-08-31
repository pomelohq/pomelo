package remote

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

type fakeDisp struct {
	queried  []string
	commands [][2]string
}

func (f *fakeDisp) Query(domain string, _ json.RawMessage) any {
	f.queried = append(f.queried, domain)
	return map[string]any{"domain": domain}
}
func (f *fakeDisp) Command(domain, action string, _ json.RawMessage) any {
	f.commands = append(f.commands, [2]string{domain, action})
	return map[string]any{"ok": true}
}
func (f *fakeDisp) Fetch(domain string, _ json.RawMessage) []byte {
	return []byte("bytes:" + domain)
}

func post(t *testing.T, h http.Handler, path, token, body string) *http.Response {
	t.Helper()
	req := httptest.NewRequest(http.MethodPost, path, strings.NewReader(body))
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec.Result()
}

func TestAuthRequired(t *testing.T) {
	h := New(&fakeDisp{}, "secret", "0.0.0").Handler()

	if r := post(t, h, "/rpc/query", "", `{"domain":"workspaces"}`); r.StatusCode != http.StatusUnauthorized {
		t.Errorf("no token: want 401, got %d", r.StatusCode)
	}
	if r := post(t, h, "/rpc/query", "wrong", `{"domain":"workspaces"}`); r.StatusCode != http.StatusUnauthorized {
		t.Errorf("bad token: want 401, got %d", r.StatusCode)
	}
	if r := post(t, h, "/rpc/query", "secret", `{"domain":"workspaces"}`); r.StatusCode != http.StatusOK {
		t.Errorf("good token: want 200, got %d", r.StatusCode)
	}
}

func TestPingIsUnauthenticated(t *testing.T) {
	h := New(&fakeDisp{}, "secret", "1.2.3").Handler()
	req := httptest.NewRequest(http.MethodGet, "/ping", nil)
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "1.2.3") {
		t.Errorf("ping should be open and report version: %d %s", rec.Code, rec.Body.String())
	}
}

func TestCommandAllowlist(t *testing.T) {
	d := &fakeDisp{}
	h := New(d, "secret", "0.0.0").Handler()

	// A mutation outside the allowlist is refused before reaching dispatch.
	if r := post(t, h, "/rpc/command", "secret", `{"domain":"service","action":"start"}`); r.StatusCode != http.StatusForbidden {
		t.Errorf("service.start: want 403, got %d", r.StatusCode)
	}
	if r := post(t, h, "/rpc/command", "secret", `{"domain":"git","action":"push"}`); r.StatusCode != http.StatusForbidden {
		t.Errorf("git.push: want 403, got %d", r.StatusCode)
	}
	if len(d.commands) != 0 {
		t.Errorf("denied commands must never reach dispatch, got %v", d.commands)
	}

	// An allowlisted mutation goes through.
	if r := post(t, h, "/rpc/command", "secret", `{"domain":"pr","action":"refresh"}`); r.StatusCode != http.StatusOK {
		t.Errorf("pr.refresh: want 200, got %d", r.StatusCode)
	}
	if len(d.commands) != 1 || d.commands[0] != [2]string{"pr", "refresh"} {
		t.Errorf("allowed command should dispatch: %v", d.commands)
	}
}

func TestQueryAndFetchReachDispatch(t *testing.T) {
	d := &fakeDisp{}
	h := New(d, "secret", "0.0.0").Handler()

	if r := post(t, h, "/rpc/query", "secret", `{"domain":"agent_states"}`); r.StatusCode != http.StatusOK {
		t.Fatalf("query status %d", r.StatusCode)
	}
	if len(d.queried) != 1 || d.queried[0] != "agent_states" {
		t.Errorf("query should reach dispatch: %v", d.queried)
	}
	r := post(t, h, "/rpc/fetch", "secret", `{"domain":"local_changes"}`)
	buf := make([]byte, 64)
	n, _ := r.Body.Read(buf)
	if got := string(buf[:n]); got != "bytes:local_changes" {
		t.Errorf("fetch body: %q", got)
	}
}
