package services

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

// OriginDefaultBranch returns the repo's default branch bare name from origin/HEAD,
// or "" when it can't be resolved (no remote HEAD ref).
func OriginDefaultBranch(wtPath string) string {
	out, err := exec.Command("git", "-C", wtPath, "symbolic-ref",
		"--short", "refs/remotes/origin/HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimPrefix(strings.TrimSpace(string(out)), "origin/")
}

func BaseRef(defaultBranch, wtPath string) string {
	branch := defaultBranch
	if branch == "" {
		branch = OriginDefaultBranch(wtPath)
	}
	if branch == "" {
		branch = "main"
	}
	if exec.Command("git", "-C", wtPath, "rev-parse", "--verify", "origin/"+branch).Run() == nil {
		return "origin/" + branch
	}
	return branch
}

func MergeBase(base, wt string) string {
	out, err := exec.Command("git", "-C", wt, "merge-base", base, "HEAD").Output()
	if err != nil {
		return base
	}
	return strings.TrimSpace(string(out))
}

// UnpushedBase returns the ref to diff HEAD against to see only unpushed
// local work: the merge-base with the branch's own upstream if one exists,
// otherwise the merge-base with defaultBranch (branch never pushed).
//
// The merge-base, not the upstream ref itself: diffing against the tip would
// report commits someone else pushed as local deletions.
func UnpushedBase(defaultBranch, wt string) string {
	if out, err := exec.Command("git", "-C", wt, "rev-parse", "--abbrev-ref",
		"--symbolic-full-name", "@{upstream}").Output(); err == nil {
		if up := strings.TrimSpace(string(out)); up != "" {
			return MergeBase(up, wt)
		}
	}
	return MergeBase(BaseRef(defaultBranch, wt), wt)
}

// FetchUpstream refreshes the branch's own remote-tracking ref so UpstreamBehind
// can see commits others pushed — origin/<branch> otherwise only moves on a
// manual fetch. Call it from a background loop; it blocks on the network.
func FetchUpstream(wt string) {
	out, err := RunTimeout(5*time.Second, wt, "git", "rev-parse", "--abbrev-ref",
		"--symbolic-full-name", "@{upstream}")
	if err != nil {
		return
	}
	remote, branch, ok := strings.Cut(strings.TrimSpace(string(out)), "/")
	if !ok || remote == "" || branch == "" {
		return
	}
	// Best-effort: offline or unreachable leaves the stale ref in place.
	_, _ = RunTimeout(20*time.Second, wt, "git", "fetch", "--quiet", remote, branch)
}

func LocalChangeStat(base, wt string) (files, insertions, deletions int) {
	if out, err := exec.Command("git", "-C", wt, "diff", "--shortstat", base).Output(); err == nil {
		files, insertions, deletions = parseShortstat(string(out))
	}
	uf, ul := untrackedStat(wt)
	files += uf
	insertions += ul
	return
}

func UntrackedFiles(wt string) []string {
	out, err := exec.Command("git", "-C", wt, "ls-files", "--others", "--exclude-standard", "-z").Output()
	if err != nil {
		return nil
	}
	var names []string
	for _, name := range strings.Split(strings.TrimRight(string(out), "\x00"), "\x00") {
		if name != "" {
			names = append(names, name)
		}
	}
	return names
}

func untrackedStat(wt string) (files, lines int) {
	for _, name := range UntrackedFiles(wt) {
		files++
		p := filepath.Join(wt, name)
		info, err := os.Stat(p)
		if err != nil || info.Size() > 2<<20 {
			continue
		}
		if data, err := os.ReadFile(p); err == nil {
			lines += bytes.Count(data, []byte{'\n'})
			if len(data) > 0 && data[len(data)-1] != '\n' {
				lines++
			}
		}
	}
	return
}

func parseShortstat(s string) (files, insertions, deletions int) {
	for _, part := range strings.Split(strings.TrimSpace(s), ",") {
		part = strings.TrimSpace(part)
		fields := strings.Fields(part)
		if len(fields) < 2 {
			continue
		}
		n, err := strconv.Atoi(fields[0])
		if err != nil {
			continue
		}
		switch {
		case strings.Contains(part, "file"):
			files = n
		case strings.Contains(part, "insertion"):
			insertions = n
		case strings.Contains(part, "deletion"):
			deletions = n
		}
	}
	return files, insertions, deletions
}

// UpstreamBehind counts commits on the branch's own upstream that HEAD lacks —
// someone else pushed to this branch and the local copy needs updating.
func UpstreamBehind(wt string) int {
	out, err := exec.Command("git", "-C", wt, "rev-list", "--count", "HEAD..@{upstream}").Output()
	if err != nil {
		return 0
	}
	n, err := strconv.Atoi(strings.TrimSpace(string(out)))
	if err != nil {
		return 0
	}
	return n
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
	// Golden source mirrors origin: hard-reset onto the fetched head so a diverged
	// main (upstream rebase/force-push, stray local commits) resolves instead of
	// dying on ff-only. Working tree was already reset+cleaned above.
	if out, err := exec.Command("git", "-C", dir, "reset", "--hard", "FETCH_HEAD").CombinedOutput(); err != nil {
		return fmt.Errorf("git reset --hard origin/%s: %s (%w)", defaultBranch, strings.TrimSpace(string(out)), err)
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
