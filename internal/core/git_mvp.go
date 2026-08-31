package core

import (
	"github.com/pomelohq/pomelo/internal/services"
)

type RepoGitStatus struct {
	Repo    string               `json:"repo"`
	Branch  string               `json:"branch"`
	Ahead   int                  `json:"ahead"`
	Behind  int                  `json:"behind"`
	Changes []services.GitChange `json:"changes"`
}

// GitWorkspaceStatus reports uncommitted changes for every materialized repo in
// the workspace, so the app can show a multi-root source-control view.
func (s *Server) GitWorkspaceStatus(branch string, isMain bool) map[string]any {
	cfg := s.cfg()
	if cfg == nil || s.WorkspaceRoot == "" {
		return map[string]any{"repos": []RepoGitStatus{}}
	}
	repos := make([]RepoGitStatus, 0, len(cfg.RepoOrder))
	for _, repo := range cfg.RepoOrder {
		wt := repoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
		if !dirExists(wt) {
			continue
		}
		ahead, behind := services.AheadBehind(cfg.DefaultBranchFor(repo), wt)
		changes := services.GitStatusPorcelain(wt)
		if changes == nil {
			changes = []services.GitChange{}
		}
		repos = append(repos, RepoGitStatus{
			Repo:    repo,
			Branch:  services.CurrentBranch(wt),
			Ahead:   ahead,
			Behind:  behind,
			Changes: changes,
		})
	}
	return map[string]any{"repos": repos}
}

func (s *Server) gitRepoWorktree(branch, repo string, isMain bool) (string, bool) {
	if s.cfg() == nil || s.WorkspaceRoot == "" {
		return "", false
	}
	wt := repoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	if !dirExists(wt) {
		return "", false
	}
	return wt, true
}

func (s *Server) GitStage(branch, repo string, isMain bool, paths []string) map[string]any {
	wt, ok := s.gitRepoWorktree(branch, repo, isMain)
	if !ok {
		return map[string]any{"ok": false, "error": "repo not in workspace"}
	}
	return okErrJSON(services.GitStage(wt, paths))
}

func (s *Server) GitUnstage(branch, repo string, isMain bool, paths []string) map[string]any {
	wt, ok := s.gitRepoWorktree(branch, repo, isMain)
	if !ok {
		return map[string]any{"ok": false, "error": "repo not in workspace"}
	}
	return okErrJSON(services.GitUnstage(wt, paths))
}

func (s *Server) GitDiscard(branch, repo string, isMain bool, paths []string) map[string]any {
	wt, ok := s.gitRepoWorktree(branch, repo, isMain)
	if !ok {
		return map[string]any{"ok": false, "error": "repo not in workspace"}
	}
	return okErrJSON(services.GitDiscard(wt, paths))
}

func (s *Server) GitCommit(branch, repo string, isMain bool, message string) map[string]any {
	wt, ok := s.gitRepoWorktree(branch, repo, isMain)
	if !ok {
		return map[string]any{"ok": false, "error": "repo not in workspace"}
	}
	out, err := services.GitCommit(wt, message)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "output": out}
	}
	return map[string]any{"ok": true, "output": out}
}

func (s *Server) GitPush(branch, repo string, isMain bool) map[string]any {
	wt, ok := s.gitRepoWorktree(branch, repo, isMain)
	if !ok {
		return map[string]any{"ok": false, "error": "repo not in workspace"}
	}
	out, err := services.GitPush(wt)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error(), "output": out}
	}
	return map[string]any{"ok": true, "output": out}
}

func okErrJSON(err error) map[string]any {
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	return map[string]any{"ok": true}
}
