package core

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
)

func TestConfigStructured(t *testing.T) {
	dir := t.TempDir()
	yml := "session: s\ndefault_branch: main\nshared_services:\n  postgres: {type: postgres}\nrepos:\n  api:\n    alias: api\n    setup: [\"go mod download\"]\n    databases: {main: api_db}\n    services:\n      web: {cmd: \"go run .\", port: true}\n"
	os.WriteFile(filepath.Join(dir, "pom.yml"), []byte(yml), 0o644)
	cfg, err := config.Load(filepath.Join(dir, "pom.yml"))
	if err != nil {
		t.Fatal(err)
	}
	s := New("", "s", dir, "main", cfg)
	ov := s.ConfigStructured()
	if ov.Session != "s" || len(ov.Repos) != 1 {
		t.Fatalf("bad overview: %+v", ov)
	}
	r := ov.Repos[0]
	if r.Name != "api" || len(r.Services) != 1 || r.Services[0].Name != "web" || !r.Services[0].Port {
		t.Fatalf("bad repo: %+v", r)
	}
	if len(r.Setup) != 1 || len(r.Databases) != 1 {
		t.Fatalf("setup/db missing: %+v", r)
	}
	if len(ov.Shared) != 1 || ov.Shared[len(ov.Shared)-1].Type != "postgres" {
		t.Fatalf("shared missing: %+v", ov.Shared)
	}
}
