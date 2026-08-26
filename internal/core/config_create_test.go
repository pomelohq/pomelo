package core

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
)

func newConfigServer(t *testing.T) (*Server, string) {
	t.Helper()
	dir := t.TempDir()
	yml := "session: s\ndefault_branch: main\nrepos:\n  a: {services: {web: {cmd: \"true\"}}}\n"
	if err := os.WriteFile(filepath.Join(dir, "pom.yml"), []byte(yml), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := config.Load(filepath.Join(dir, "pom.yml"))
	if err != nil {
		t.Fatal(err)
	}
	return New("", "s", dir, "main", cfg), dir
}

func TestConfigFileCreate(t *testing.T) {
	s, dir := newConfigServer(t)

	r := s.ConfigFileCreate("services.yml", "")
	if ok, _ := r["ok"].(bool); !ok {
		t.Fatalf("create failed: %+v", r)
	}
	if _, err := os.Stat(filepath.Join(dir, "pom.d", "services.yml")); err != nil {
		t.Fatalf("file not written: %v", err)
	}

	// nested subdir under pom.d is allowed
	if ok, _ := s.ConfigFileCreate("backend/api.yml", "")["ok"].(bool); !ok {
		t.Fatal("nested create should succeed")
	}

	// re-create the same file fails
	if ok, _ := s.ConfigFileCreate("services.yml", "")["ok"].(bool); ok {
		t.Fatal("recreating an existing file should fail")
	}
}

func TestConfigFileCreateRejectsBadNames(t *testing.T) {
	s, _ := newConfigServer(t)
	for _, name := range []string{"../evil.yml", "notyaml.txt", "", "../../etc/passwd"} {
		if ok, _ := s.ConfigFileCreate(name, "")["ok"].(bool); ok {
			t.Fatalf("should reject %q", name)
		}
	}
}
