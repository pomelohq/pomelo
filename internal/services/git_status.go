package services

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// GitChange is one entry from `git status`: a path plus its index (X) and
// worktree (Y) porcelain codes, flattened into flags the UI groups by.
type GitChange struct {
	Path      string `json:"path"`
	Orig      string `json:"orig,omitempty"`
	Index     string `json:"index"`
	Worktree  string `json:"worktree"`
	Staged    bool   `json:"staged"`
	Unstaged  bool   `json:"unstaged"`
	Untracked bool   `json:"untracked"`
}

func GitStatusPorcelain(wt string) []GitChange {
	out, err := RunTimeout(10*time.Second, wt, "git", "status", "--porcelain=v1", "-z")
	if err != nil {
		return nil
	}
	return parsePorcelainZ(string(out))
}

// parsePorcelainZ decodes `git status --porcelain=v1 -z`. Records are NUL-
// separated; a rename/copy record is followed by a second NUL field carrying
// the original path.
func parsePorcelainZ(s string) []GitChange {
	toks := strings.Split(s, "\x00")
	changes := make([]GitChange, 0, len(toks))
	for i := 0; i < len(toks); i++ {
		rec := toks[i]
		if len(rec) < 4 {
			continue
		}
		x, y := rec[0], rec[1]
		c := GitChange{Path: rec[3:], Index: string(x), Worktree: string(y)}
		switch {
		case x == '?' && y == '?':
			c.Untracked = true
			c.Unstaged = true
		default:
			if x != ' ' && x != '?' {
				c.Staged = true
			}
			if y != ' ' && y != '?' {
				c.Unstaged = true
			}
		}
		if x == 'R' || x == 'C' {
			if i+1 < len(toks) {
				c.Orig = toks[i+1]
				i++
			}
		}
		changes = append(changes, c)
	}
	return changes
}

func GitStage(wt string, paths []string) error {
	if len(paths) == 0 {
		return RunGit(wt, "add", "-A")
	}
	return RunGit(wt, append([]string{"add", "--"}, paths...)...)
}

func GitUnstage(wt string, paths []string) error {
	if len(paths) == 0 {
		return RunGit(wt, "reset", "--quiet")
	}
	return RunGit(wt, append([]string{"restore", "--staged", "--"}, paths...)...)
}

// GitDiscard reverts tracked paths to HEAD and deletes untracked ones. It reads
// the current status to tell the two apart, so a caller only passes paths.
func GitDiscard(wt string, paths []string) error {
	if len(paths) == 0 {
		return fmt.Errorf("no paths to discard")
	}
	untracked := map[string]bool{}
	for _, c := range GitStatusPorcelain(wt) {
		if c.Untracked {
			untracked[c.Path] = true
		}
	}
	var tracked []string
	for _, p := range paths {
		if untracked[p] {
			if err := os.Remove(filepath.Join(wt, p)); err != nil && !os.IsNotExist(err) {
				return err
			}
			continue
		}
		tracked = append(tracked, p)
	}
	if len(tracked) > 0 {
		return RunGit(wt, append([]string{"restore", "--staged", "--worktree", "--"}, tracked...)...)
	}
	return nil
}

func GitCommit(wt, message string) (string, error) {
	if strings.TrimSpace(message) == "" {
		return "", fmt.Errorf("commit message is empty")
	}
	out, err := RunTimeout(30*time.Second, wt, "git", "commit", "-m", message)
	return string(out), err
}

// GitPush pushes the current branch, setting upstream on first push.
func GitPush(wt string) (string, error) {
	if _, err := RunTimeout(5*time.Second, wt, "git", "rev-parse", "--abbrev-ref",
		"--symbolic-full-name", "@{upstream}"); err == nil {
		out, err := RunTimeout(2*time.Minute, wt, "git", "push")
		return string(out), err
	}
	branch := CurrentBranch(wt)
	if branch == "" {
		return "", fmt.Errorf("cannot resolve current branch")
	}
	out, err := RunTimeout(2*time.Minute, wt, "git", "push", "-u", "origin", branch)
	return string(out), err
}

func RunGit(wt string, args ...string) error {
	if out, err := RunTimeout(30*time.Second, wt, "git", args...); err != nil {
		return fmt.Errorf("git %s: %w: %s", strings.Join(args, " "), err, strings.TrimSpace(string(out)))
	}
	return nil
}
