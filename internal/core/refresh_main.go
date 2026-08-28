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

// refreshScheduler runs the golden-source pull on a cron-aligned cadence; SyncSet
// pokes syncReset to reschedule without an app restart.
func (s *Server) refreshScheduler() {
	pull := func() {
		s.syncPulling.Store(true)
		s.refreshMain()
		s.syncPulling.Store(false)
		s.syncLastPull.Store(time.Now().Unix())
	}
	firstRun := true
	for {
		rm, intervalSec := s.effectiveSync()
		if !rm {
			s.syncNextAt.Store(0)
			firstRun = true
			<-s.syncReset
			continue
		}
		interval := 1800
		if intervalSec > 0 {
			interval = intervalSec
		}
		// Pull once on startup / when sync is enabled so the user sees it work now and
		// main lands on its default branch immediately, instead of waiting a full cycle.
		if firstRun {
			firstRun = false
			pull()
		}
		next := nextAlignedRun(time.Now(), interval)
		s.syncNextAt.Store(next.Unix())
		select {
		case <-time.After(time.Until(next)):
			pull()
		case <-s.syncReset:
		}
	}
}

// nextAlignedRun picks the next wall-clock boundary within the hour, like cron `*/N`.
func nextAlignedRun(now time.Time, intervalSec int) time.Time {
	n := intervalSec / 60
	if n < 1 {
		n = 1
	}
	hourStart := time.Date(now.Year(), now.Month(), now.Day(), now.Hour(), 0, 0, 0, now.Location())
	if n >= 60 {
		return hourStart.Add(time.Hour)
	}
	k := int(now.Sub(hourStart).Minutes())/n + 1
	if k*n >= 60 {
		return hourStart.Add(time.Hour)
	}
	return hourStart.Add(time.Duration(k*n) * time.Minute)
}

func (s *Server) refreshMain() {
	cfg := s.cfg()
	refreshMain, _ := s.effectiveSync()
	if cfg == nil || !refreshMain || s.WorkspaceRoot == "" {
		return
	}
	// Each repo resolves its own default branch (a multi-repo project mixes main and
	// master); origin/HEAD is the source of truth, config is the fallback.
	repoDefault := func(repo, wt string) string {
		if db := services.OriginDefaultBranch(wt); db != "" {
			return db
		}
		return cfg.DefaultBranchFor(repo)
	}

	var prog []RepoPull
	idx := map[string]int{}
	for _, repo := range cfg.RepoOrder {
		if dir := cfg.Repos[repo]; dir != nil && dirExists(repoWorktreePath(s.WorkspaceRoot, repo, "", true)) {
			idx[repo] = len(prog)
			prog = append(prog, RepoPull{Repo: repo, State: "pending"})
		}
	}
	publish := func() { cp := append([]RepoPull(nil), prog...); s.syncProg.Store(&cp) }
	set := func(repo, state string) {
		if i, ok := idx[repo]; ok {
			prog[i].State = state
			prog[i].Detail = ""
			publish()
		}
	}
	fail := func(repo, detail string) {
		if i, ok := idx[repo]; ok {
			prog[i].State = "failed"
			prog[i].Detail = detail
			publish()
		}
	}
	publish()

	for _, repo := range cfg.RepoOrder {
		dir := cfg.Repos[repo]
		if dir == nil {
			continue
		}
		wt := repoWorktreePath(s.WorkspaceRoot, repo, "", true)
		if !dirExists(wt) {
			continue
		}
		defBranch := repoDefault(repo, wt)
		set(repo, "pulling")
		// The main workspace must keep every repo on its default branch. Respect local
		// WIP (skip) only when already on default; a repo parked on a feature branch is
		// forced back regardless.
		onDefault := services.CurrentBranch(wt) == defBranch
		dirty := func() bool {
			out, _ := services.RunTimeout(10*time.Second, wt, "git", "status", "--porcelain")
			return len(strings.TrimSpace(string(out))) > 0
		}()
		if onDefault && dirty {
			set(repo, "skipped")
			continue
		}
		before, _ := services.RunTimeout(10*time.Second, wt, "git", "rev-parse", "HEAD")
		// Same path as the manual "Update main from origin": mirror origin (handles a
		// diverged main), not a plain ff-only pull that skips on divergence.
		if err := services.ResetToDefaultAndPull(wt, defBranch); err != nil {
			fail(repo, lastLines(err.Error(), 3))
			continue
		}
		after, _ := services.RunTimeout(10*time.Second, wt, "git", "rev-parse", "HEAD")
		if strings.TrimSpace(string(before)) == strings.TrimSpace(string(after)) {
			set(repo, "nochange")
			continue
		}
		if mig := dir.EffectiveMigrate(); len(mig) > 0 {
			set(repo, "migrating")
			s.runMainMigrate(defBranch, repo, wt, mig)
		}
		set(repo, "updated")
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
