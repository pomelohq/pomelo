package core

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestListWorkspaceFilesAndReadFile(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "api", "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "api", "main.go"), []byte("package main\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "api", "src", "a.go"), []byte("package src\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(root, "api", ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "api", ".git", "HEAD"), []byte("ref: refs/heads/main\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	s := &Server{WorkspaceRoot: root}

	var entries []FileEntry
	if err := json.Unmarshal(s.ListWorkspaceFiles("main", true), &entries); err != nil {
		t.Fatalf("decode: %v", err)
	}
	for _, e := range entries {
		if e.Repo != "api" {
			t.Fatalf("unexpected repo %q", e.Repo)
		}
		if e.Path == ".git" || filepath.Base(e.Path) == "HEAD" {
			t.Fatalf(".git contents leaked into listing: %+v", e)
		}
	}
	found := map[string]bool{}
	for _, e := range entries {
		found[e.Path] = true
	}
	if !found["main.go"] || !found["src/a.go"] || !found["src"] {
		t.Fatalf("missing expected entries: %+v", entries)
	}

	var fc FileContent
	if err := json.Unmarshal(s.ReadFile("main", "api", "main.go", true), &fc); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if fc.Text != "package main\n" || fc.Binary {
		t.Fatalf("bad file content: %+v", fc)
	}

	var errResp struct{ Error string `json:"error"` }
	if err := json.Unmarshal(s.ReadFile("main", "api", "../etc/passwd", true), &errResp); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if errResp.Error == "" {
		t.Fatal("expected path traversal to be rejected")
	}
}

func TestReadFileImage(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "web"), 0o755); err != nil {
		t.Fatal(err)
	}
	png := []byte{0x89, 'P', 'N', 'G', 0x0d, 0x0a, 0x1a, 0x0a}
	if err := os.WriteFile(filepath.Join(root, "web", "logo.png"), png, 0o644); err != nil {
		t.Fatal(err)
	}

	s := &Server{WorkspaceRoot: root}
	var fc FileContent
	if err := json.Unmarshal(s.ReadFile("main", "web", "logo.png", true), &fc); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if fc.MimeType != "image/png" || fc.Base64 == "" {
		t.Fatalf("bad image content: %+v", fc)
	}
}
