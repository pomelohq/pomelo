package core

import (
	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/provider/forge"
)

func cfgDefaultBranch(cfg *config.Config, repo string) string {
	if cfg == nil {
		return ""
	}
	return cfg.DefaultBranchFor(repo)
}

func (s *Server) GithubTest(token string) map[string]any { return forge.GithubTest(token) }

func (s *Server) PRRefresh() map[string]any { s.pr.InvalidateAll(); return map[string]any{"ok": true} }

func (s *Server) PRAllPRs() []byte { return s.pr.AllPRs() }
func (s *Server) PRWorkspacePRs(branch string, isMain bool) []byte {
	return s.pr.WorkspacePRs(branch, isMain)
}
func (s *Server) PRDetail(branch, repo string, isMain bool) []byte {
	return s.pr.PRDetail(branch, repo, isMain)
}
func (s *Server) PRComments(branch, repo string, isMain bool) []byte {
	return s.pr.PRComments(branch, repo, isMain)
}
func (s *Server) PRTimeline(branch, repo string, isMain bool) []byte {
	return s.pr.Timeline(branch, repo, isMain)
}
func (s *Server) PRCommits(branch, repo, base string, isMain bool) map[string]any {
	return s.pr.RepoCommits(branch, repo, base, isMain)
}
func (s *Server) PRDiff(branch, repo string, isMain bool) ([]byte, error) {
	return s.pr.Diff(branch, repo, isMain)
}
func (s *Server) WorkspaceLocalChanges(branch string, isMain bool) []byte {
	return s.pr.WorkspaceLocalChanges(branch, isMain)
}
func (s *Server) LocalDiff(branch, repo string, isMain bool) ([]byte, error) {
	return s.pr.LocalDiff(branch, repo, isMain)
}
