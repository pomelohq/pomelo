package core

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// The first session is created before any server exists (no project loaded yet), so
// ScaffoldSessionCmd must run standalone — regression guard for the "no server" bug.
func TestScaffoldSessionCmdNoServer(t *testing.T) {
	if testing.Short() {
		t.Skip("shells out to git")
	}
	t.Setenv("POM_SESSIONS_ROOT", t.TempDir())

	repo := filepath.Join(t.TempDir(), "myrepo")
	if err := os.MkdirAll(repo, 0o755); err != nil {
		t.Fatal(err)
	}
	run := func(args ...string) {
		c := exec.Command("git", args...)
		c.Dir = repo
		if out, err := c.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-b", "main")
	run("config", "user.email", "t@example.com")
	run("config", "user.name", "t")
	if err := os.WriteFile(filepath.Join(repo, "README.md"), []byte("hi"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-m", "init")

	req := CreateSessionReq{Name: "boomtest", DefaultBranch: "main", Repos: []RepoSpec{{Path: repo, Alias: "myrepo"}}}
	body, _ := json.Marshal(req)
	res := ScaffoldSessionCmd(json.RawMessage(body))

	m, ok := res.(map[string]any)
	if !ok || m["ok"] != true {
		t.Fatalf("expected ok, got %#v", res)
	}
	if p, _ := m["path"].(string); !strings.Contains(p, "boomtest") {
		t.Fatalf("bad path in result: %#v", res)
	}
}
