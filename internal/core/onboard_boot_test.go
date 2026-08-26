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

// TestVerifyBootDetectsCrash spins two real ptyhost holders — one that stays up,
// one that exits immediately — and asserts verifyBoot flags only the crasher.
// It builds a real pom binary so the holders can re-exec `pom pty run`.
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
