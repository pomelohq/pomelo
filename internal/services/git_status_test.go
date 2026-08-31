package services

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestParsePorcelainZ(t *testing.T) {
	// modified-staged, modified-unstaged, untracked, and a rename (with orig).
	in := "M  staged.txt\x00 M dirty.txt\x00?? new.txt\x00R  to.txt\x00from.txt\x00"
	got := parsePorcelainZ(in)
	if len(got) != 4 {
		t.Fatalf("want 4 changes, got %d: %+v", len(got), got)
	}
	if !got[0].Staged || got[0].Unstaged || got[0].Path != "staged.txt" {
		t.Errorf("staged.txt: %+v", got[0])
	}
	if got[1].Staged || !got[1].Unstaged || got[1].Path != "dirty.txt" {
		t.Errorf("dirty.txt: %+v", got[1])
	}
	if !got[2].Untracked || got[2].Path != "new.txt" {
		t.Errorf("new.txt: %+v", got[2])
	}
	if got[3].Path != "to.txt" || got[3].Orig != "from.txt" || !got[3].Staged {
		t.Errorf("rename: %+v", got[3])
	}
}

func gitMutRepo(t *testing.T) string {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not available")
	}
	repo := t.TempDir()
	gitRun(t, repo, "init", "-q")
	gitRun(t, repo, "branch", "-M", "main")
	if err := os.WriteFile(filepath.Join(repo, "base.txt"), []byte("base\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitRun(t, repo, "add", ".")
	gitRun(t, repo, "commit", "-q", "-m", "init")
	return repo
}

func TestGitStageUnstageDiscardCommit(t *testing.T) {
	repo := gitMutRepo(t)
	write := func(name, body string) {
		if err := os.WriteFile(filepath.Join(repo, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}

	write("base.txt", "changed\n")
	write("added.txt", "new\n")

	if err := GitStage(repo, []string{"base.txt"}); err != nil {
		t.Fatalf("stage: %v", err)
	}
	st := GitStatusPorcelain(repo)
	if len(st) != 2 {
		t.Fatalf("want 2 entries, got %+v", st)
	}
	var base, added GitChange
	for _, c := range st {
		switch c.Path {
		case "base.txt":
			base = c
		case "added.txt":
			added = c
		}
	}
	if !base.Staged {
		t.Errorf("base.txt should be staged: %+v", base)
	}
	if !added.Untracked {
		t.Errorf("added.txt should be untracked: %+v", added)
	}

	if err := GitUnstage(repo, []string{"base.txt"}); err != nil {
		t.Fatalf("unstage: %v", err)
	}
	for _, c := range GitStatusPorcelain(repo) {
		if c.Path == "base.txt" && c.Staged {
			t.Errorf("base.txt should be unstaged after unstage: %+v", c)
		}
	}

	// discard reverts the tracked edit and deletes the untracked file.
	if err := GitDiscard(repo, []string{"base.txt", "added.txt"}); err != nil {
		t.Fatalf("discard: %v", err)
	}
	if st := GitStatusPorcelain(repo); len(st) != 0 {
		t.Errorf("want clean tree after discard, got %+v", st)
	}
	if _, err := os.Stat(filepath.Join(repo, "added.txt")); !os.IsNotExist(err) {
		t.Errorf("added.txt should be deleted by discard")
	}

	// commit an empty message must fail; a real one must succeed.
	if _, err := GitCommit(repo, "  "); err == nil {
		t.Error("empty commit message should error")
	}
	write("base.txt", "again\n")
	if err := GitStage(repo, []string{"base.txt"}); err != nil {
		t.Fatal(err)
	}
	if _, err := GitCommit(repo, "edit base"); err != nil {
		t.Fatalf("commit: %v", err)
	}
	if st := GitStatusPorcelain(repo); len(st) != 0 {
		t.Errorf("tree should be clean after commit, got %+v", st)
	}
}
