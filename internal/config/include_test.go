package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestFragmentMerge(t *testing.T) {
	dir := t.TempDir()
	root := `session: demo
default_branch: main
repos:
  api:
    alias: api
    services:
      s:
        cmd: "go run ."
`
	if err := os.WriteFile(filepath.Join(dir, "pom.yml"), []byte(root), 0o644); err != nil {
		t.Fatal(err)
	}
	fragDir := filepath.Join(dir, "pom.d")
	if err := os.MkdirAll(fragDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(fragDir, "10-web.yml"), []byte(
		"repos:\n  web:\n    alias: web\n    services:\n      dev:\n        cmd: vite\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(fragDir, "20-shared.yml"), []byte(
		"shared_services:\n  postgres:\n    image: postgres:16\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	cfg, err := Load(filepath.Join(dir, "pom.yml"))
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Repos["api"] == nil || cfg.Repos["web"] == nil {
		t.Fatalf("merged repos missing: %v", cfg.Repos)
	}
	if cfg.SharedServices["postgres"] == nil {
		t.Fatalf("merged shared_services missing")
	}
	if len(cfg.RepoOrder) < 2 || cfg.RepoOrder[0] != "api" || cfg.RepoOrder[1] != "web" {
		t.Fatalf("repo order not preserved: %v", cfg.RepoOrder)
	}
}

func TestSplitToFragmentsRoundTrip(t *testing.T) {
	dir := t.TempDir()
	orig := `session: demo
default_branch: main
repos:
  api:
    alias: api
    services:
      s:
        cmd: "go run ."
  web:
    alias: web
    services:
      dev:
        cmd: vite
`
	src := filepath.Join(dir, "pom.yml")
	if err := os.WriteFile(src, []byte(orig), 0o644); err != nil {
		t.Fatal(err)
	}

	res, err := SplitToFragments(src, false)
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Base(res.RootFile) != "pom.yml" {
		t.Fatalf("expected root pom.yml, got %+v", res)
	}
	if len(res.Fragments) != 2 {
		t.Fatalf("expected 2 repo fragments, got %v", res.Fragments)
	}
	if _, err := os.Stat(res.BackupFile); err != nil {
		t.Fatalf("backup missing: %v", err)
	}

	cfg, err := Load(res.RootFile)
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Repos["api"] == nil || cfg.Repos["web"] == nil {
		t.Fatalf("repos lost after split: %v", cfg.Repos)
	}
	if cfg.DefaultBranch != "main" {
		t.Fatal("non-repo config lost after split")
	}
}

func TestNoFragmentDirUnchanged(t *testing.T) {
	dir := t.TempDir()
	root := "session: demo\nrepos:\n  api:\n    services:\n      s:\n        cmd: x\n"
	p := filepath.Join(dir, "pom.yml")
	if err := os.WriteFile(p, []byte(root), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := Load(p)
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Repos["api"] == nil {
		t.Fatal("plain single-file load broke")
	}
}
