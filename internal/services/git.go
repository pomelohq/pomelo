package services

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"time"
)

func BaseRef(defaultBranch, wtPath string) string {
	branch := defaultBranch
	if branch == "" {
		if out, err := exec.Command("git", "-C", wtPath, "symbolic-ref",
			"--short", "refs/remotes/origin/HEAD").Output(); err == nil {
			s := strings.TrimPrefix(strings.TrimSpace(string(out)), "origin/")
			if s != "" {
				branch = s
			}
		}
	}
	if branch == "" {
		branch = "main"
	}
	if exec.Command("git", "-C", wtPath, "rev-parse", "--verify", "origin/"+branch).Run() == nil {
		return "origin/" + branch
	}
	return branch
}

func AheadBehind(defaultBranch, wt string) (ahead, behind int) {
	base := BaseRef(defaultBranch, wt)
	out, err := exec.Command("git", "-C", wt, "rev-list", "--left-right", "--count", base+"...HEAD").Output()
	if err != nil {
		return 0, 0
	}
	f := strings.Fields(strings.TrimSpace(string(out)))
	if len(f) != 2 {
		return 0, 0
	}
	behind, _ = strconv.Atoi(f[0])
	ahead, _ = strconv.Atoi(f[1])
	return ahead, behind
}

func ListWorktrees(dir string) []GitWorktree {
	out, err := exec.Command("git", "-C", dir, "worktree", "list", "--porcelain").Output()
	if err != nil {
		return nil
	}

	var result []GitWorktree
	var currentPath, currentBranch string
	for _, line := range strings.Split(string(out), "\n") {
		if path, ok := strings.CutPrefix(line, "worktree "); ok {
			currentPath = path
		} else if branch, ok := strings.CutPrefix(line, "branch refs/heads/"); ok {
			currentBranch = branch
		} else if line == "" && currentPath != "" {
			if currentBranch != "" {
				result = append(result, GitWorktree{Path: currentPath, Branch: currentBranch})
			}
			currentPath = ""
			currentBranch = ""
		}
	}
	if currentPath != "" && currentBranch != "" {
		result = append(result, GitWorktree{Path: currentPath, Branch: currentBranch})
	}
	return result
}

func CurrentBranch(dir string) string {
	out, err := exec.Command("git", "-C", dir, "rev-parse", "--abbrev-ref", "HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func CreateWorktreeFromBase(repoDir, newBranch, baseBranch string, copyFilesList []string, workspaceDir string) (string, error) {
	repoName := fileBase(repoDir)

	var worktreeDir string
	if workspaceDir != "" {
		worktreeDir = workspaceDir + "/" + repoName
	} else {
		dirSuffix := strings.ReplaceAll(newBranch, "/", "-")
		worktreeDir = repoDir + "/../" + repoName + "--" + dirSuffix
	}

	if _, err := os.Stat(worktreeDir); err == nil {
		return "", fmt.Errorf("worktree directory already exists: %s", worktreeDir)
	}

	_ = exec.Command("git", "-C", repoDir, "worktree", "prune").Run()

	branchExists := exec.Command("git", "-C", repoDir, "rev-parse", "--verify", newBranch).Run() == nil
	remoteExists := !branchExists && exec.Command("git", "-C", repoDir, "rev-parse", "--verify", "origin/"+newBranch).Run() == nil

	build := func() *exec.Cmd {
		switch {
		case branchExists:
			return exec.Command("git", "-C", repoDir, "worktree", "add", worktreeDir, newBranch)
		case remoteExists:
			return exec.Command("git", "-C", repoDir, "worktree", "add", "--track", "-b", newBranch, worktreeDir, "origin/"+newBranch)
		default:
			return exec.Command("git", "-C", repoDir, "worktree", "add", "-b", newBranch, worktreeDir, baseBranch)
		}
	}

	out, err := build().CombinedOutput()
	if err != nil && strings.Contains(string(out), "already used by worktree") {
		_ = exec.Command("git", "-C", repoDir, "worktree", "remove", "--force", worktreeDir).Run()
		_ = exec.Command("git", "-C", repoDir, "worktree", "prune").Run()
		out, err = build().CombinedOutput()
	}
	if err != nil {
		return "", fmt.Errorf("git worktree add: %s (%w)", strings.TrimSpace(string(out)), err)
	}

	CopyFiles(repoDir, worktreeDir, copyFilesList)
	return worktreeDir, nil
}

func RemoveWorktree(repoDir, worktreePath, branch string) error {
	_ = exec.Command("git", "-C", repoDir, "worktree", "remove", "--force", worktreePath).Run()

	if _, err := os.Stat(worktreePath); err == nil {
		_ = os.RemoveAll(worktreePath)
	}

	_ = exec.Command("git", "-C", repoDir, "worktree", "prune").Run()
	_ = exec.Command("git", "-C", repoDir, "branch", "-D", branch).Run()
	return nil
}

func Checkout(dir, branch string) error {
	out, err := exec.Command("git", "-C", dir, "checkout", branch).CombinedOutput()
	if err != nil {
		return fmt.Errorf("git checkout %s: %s (%w)", branch, strings.TrimSpace(string(out)), err)
	}
	return nil
}

func CheckoutNewBranch(dir, branch string) error {
	out, err := exec.Command("git", "-C", dir, "checkout", "-b", branch).CombinedOutput()
	if err != nil {
		return fmt.Errorf("git checkout -b %s: %s (%w)", branch, strings.TrimSpace(string(out)), err)
	}
	return nil
}

func CreateWorktree(repoDir, branch string, copyFilesList []string) (string, error) {
	dirSuffix := strings.ReplaceAll(branch, "/", "-")
	repoName := fileBase(repoDir)
	parent := repoDir + "/.."
	worktreeDir := parent + "/" + repoName + "--" + dirSuffix

	if _, err := os.Stat(worktreeDir); err == nil {
		return "", fmt.Errorf("worktree directory already exists: %s", worktreeDir)
	}

	out, err := exec.Command("git", "-C", repoDir, "worktree", "add", worktreeDir, branch).CombinedOutput()
	if err != nil {
		stderr := string(out)
		if strings.Contains(stderr, "already checked out") || strings.Contains(stderr, "is already used") {
			newBranch := "wt/" + dirSuffix
			out2, err2 := exec.Command("git", "-C", repoDir, "worktree", "add", worktreeDir, newBranch).CombinedOutput()
			if err2 != nil {
				out3, err3 := exec.Command("git", "-C", repoDir, "worktree", "add", "-b", newBranch, worktreeDir, branch).CombinedOutput()
				if err3 != nil {
					return "", fmt.Errorf("git worktree add failed: %s", strings.TrimSpace(string(out3)))
				}
				_ = out2
			}
		} else {
			return "", fmt.Errorf("git worktree add failed: %s", strings.TrimSpace(stderr))
		}
	}

	CopyFiles(repoDir, worktreeDir, copyFilesList)
	return worktreeDir, nil
}

func ResetToDefaultAndPull(dir, defaultBranch string) error {
	for _, args := range [][]string{{"reset", "--hard"}, {"clean", "-fd"}, {"checkout", defaultBranch}} {
		full := append([]string{"-C", dir}, args...)
		if out, err := exec.Command("git", full...).CombinedOutput(); err != nil {
			return fmt.Errorf("git %s: %s (%w)", strings.Join(args, " "), strings.TrimSpace(string(out)), err)
		}
	}
	// Fetch then ff-merge the single FETCH_HEAD; `git pull` on some repos multiplies
	// merge heads and dies "cannot fast-forward to multiple branches".
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()
	if out, err := exec.CommandContext(ctx, "git", "-C", dir, "fetch", "origin", defaultBranch).CombinedOutput(); err != nil {
		return fmt.Errorf("git fetch origin %s: %s (%w)", defaultBranch, strings.TrimSpace(string(out)), err)
	}
	if out, err := exec.Command("git", "-C", dir, "merge", "--ff-only", "FETCH_HEAD").CombinedOutput(); err != nil {
		return fmt.Errorf("git merge --ff-only origin/%s: %s (%w)", defaultBranch, strings.TrimSpace(string(out)), err)
	}
	return nil
}

func fileBase(path string) string {
	parts := strings.Split(strings.TrimRight(path, "/"), "/")
	if len(parts) == 0 {
		return "repo"
	}
	return parts[len(parts)-1]
}
