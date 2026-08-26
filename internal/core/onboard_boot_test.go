package core

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/pombin"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

func boolPtr(b bool) *bool { return &b }

// Spins two real ptyhost holders — one stays up, one exits — and asserts
// verifyBoot flags only the crasher. Builds pom so holders can re-exec it.
func TestVerifyBootDetectsCrash(t *testing.T) {
	if testing.Short() {
		t.Skip("integration test spawns real subprocesses")
	}
	bin := filepath.Join(t.TempDir(), "pom")
	if out, err := exec.Command("go", "build", "-o", bin, "../../cmd/pom").CombinedOutput(); err != nil {
		t.Skipf("cannot build pom: %v\n%s", err, out)
	}
	pombin.Set(bin)

	root := t.TempDir()
	const repo = "app"
	if err := os.MkdirAll(filepath.Join(root, repo), 0o755); err != nil {
		t.Fatal(err)
	}

	cfg := &config.Config{
		Session: "boottest",
		Repos: map[string]*config.Dir{
			repo: {Services: map[string]*config.Service{
				"good": {Cmd: "sleep 60", Port: boolPtr(false)},
				"bad":  {Cmd: "exit 1", Port: boolPtr(false)},
			}},
		},
	}
	s := New("", root, root, "main", cfg)

	t.Cleanup(func() {
		for _, sv := range []string{"good", "bad"} {
			_ = ptyhost.KillHolder(services.ServiceHolderName(cfg.Session, "main", repo, sv))
		}
	})

	errs := s.verifyBoot("main", true, 3*time.Second)

	if len(errs) != 1 {
		t.Fatalf("expected 1 boot failure, got %d: %+v", len(errs), errs)
	}
	if errs[0].ID != "boot.app/bad" {
		t.Fatalf("expected the crashing service flagged, got %q", errs[0].ID)
	}
	if !ptyhost.HolderAlive(services.ServiceHolderName(cfg.Session, "main", repo, "good")) {
		t.Fatalf("healthy service should still be alive")
	}
}

func TestReloadConfigPicksUpEdits(t *testing.T) {
	dir := t.TempDir()
	seed := "session: s\ndefault_branch: main\nrepos:\n  a: {services: {old: {cmd: \"true\"}}}\n"
	if err := os.WriteFile(filepath.Join(dir, "pom.yml"), []byte(seed), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := config.Load(filepath.Join(dir, "pom.yml"))
	if err != nil {
		t.Fatal(err)
	}
	s := New("", "s", dir, "main", cfg)
	edited := "session: s\ndefault_branch: main\nrepos:\n  a: {services: {web: {cmd: \"true\"}}}\n"
	if err := os.WriteFile(filepath.Join(dir, "pom.yml"), []byte(edited), 0o644); err != nil {
		t.Fatal(err)
	}
	s.reloadConfig()
	if _, ok := s.cfg().Repos["a"].Services["web"]; !ok {
		t.Fatalf("reload did not pick up the edited service: %+v", s.cfg().Repos["a"].Services)
	}
	if _, ok := s.cfg().Repos["a"].Services["old"]; ok {
		t.Fatalf("stale service still present after reload")
	}
}
