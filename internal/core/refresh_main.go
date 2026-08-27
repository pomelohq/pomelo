package core

import (
	"github.com/pomelohq/pomelo/internal/provider/shell"
	"log"
	"strings"
	"time"

	"github.com/pomelohq/pomelo/internal/appstate"
	"github.com/pomelohq/pomelo/internal/services"
)

func (s *Server) effectiveSync() (refreshMain bool, intervalSec int) {
	if sy := appstate.Load(s.session()).Sync; sy.Configured {
		return sy.RefreshMain, sy.RefreshIntervalSec
	}
	if cfg := s.cfg(); cfg != nil && cfg.Sync != nil {
		return cfg.Sync.RefreshMain, cfg.Sync.RefreshIntervalSec
	}
	return false, 0
}

func (s *Server) refreshMainLoop() {
	_, intervalSec := s.effectiveSync()
	interval := 1800
	if intervalSec > 0 {
		interval = intervalSec
	}
	t := time.NewTicker(time.Duration(interval) * time.Second)
	defer t.Stop()
	s.refreshMain()
	for range t.C {
		s.refreshMain()
	}
}

func (s *Server) refreshMain() {
	cfg := s.cfg()
	refreshMain, _ := s.effectiveSync()
	if cfg == nil || !refreshMain || s.WorkspaceRoot == "" {
		return
	}
	defBranch := cfg.GlobalDefaultBranch()
	for _, repo := range cfg.RepoOrder {
		dir := cfg.Repos[repo]
		if dir == nil {
			continue
		}
		wt := repoWorktreePath(s.WorkspaceRoot, repo, defBranch, true)
		if !dirExists(wt) {
			continue
		}
		if out, _ := services.RunTimeout(10*time.Second, wt, "git", "status", "--porcelain"); len(strings.TrimSpace(string(out))) > 0 {
			log.Printf("refresh_main: %s has local changes — skipped", repo)
			continue
		}
		before, _ := services.RunTimeout(10*time.Second, wt, "git", "rev-parse", "HEAD")
		// Same path as the manual "Update main from origin": mirror origin (handles a
		// diverged main), not a plain ff-only pull that skips on divergence.
		if err := services.ResetToDefaultAndPull(wt, defBranch); err != nil {
			log.Printf("refresh_main: %s sync failed — skipped: %v", repo, err)
			continue
		}
		after, _ := services.RunTimeout(10*time.Second, wt, "git", "rev-parse", "HEAD")
		if strings.TrimSpace(string(before)) == strings.TrimSpace(string(after)) {
			continue
		}
		mig := dir.EffectiveMigrate()
		if len(mig) == 0 {
			log.Printf("refresh_main: %s updated (no migrate command — pull only)", repo)
			continue
		}
		s.runMainMigrate(defBranch, repo, wt, mig)
	}
}

func (s *Server) runMainMigrate(branch, repo, wt string, cmds []string) {
	services.RegenerateWorkspaceEnv(s.WorkspaceRoot, s.cfg(), branch)
	env := services.ResolveRepoEnv(s.WorkspaceRoot, s.cfg(), branch, repo)
	login := shell.Login(strings.Join(cmds, " && "))
	if out, err := services.RunTimeoutEnv(5*time.Minute, wt, env, login[0], login[1:]...); err != nil {
		log.Printf("refresh_main: %s migrate failed: %v\n%s", repo, err, string(out))
		return
	}
	log.Printf("refresh_main: %s pulled + migrated", repo)
}
