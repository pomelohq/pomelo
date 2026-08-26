package core

import (
	"time"

	"github.com/pomelohq/pomelo/internal/doctor"
	"github.com/pomelohq/pomelo/internal/provider/shell"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

// InstallDeps runs every repo's setup (install) once and reports failures, so the
// app can install dependencies after onboarding (otherwise Start hits
// command-not-found). Returns {ok, failed:[{id,title,detail}]}.
func (s *Server) InstallDeps(branch string, isMain bool) map[string]any {
	errs := s.runSetup(branch, isMain, map[string]bool{})
	// Bring shared infra + per-branch DBs up so migrations have a database to hit,
	// then run each repo's migrate (a fresh DB otherwise throws "table not found").
	if cfg := s.cfg(); cfg != nil && len(cfg.SharedServices) > 0 {
		s.ensureSharedServices()
		services.EnsureWorkspaceDatabases(cfg, branch)
	}
	errs = append(errs, s.runMigrate(branch, isMain)...)
	failed := make([]map[string]string, 0, len(errs))
	for _, f := range errs {
		failed = append(failed, map[string]string{"id": f.ID, "title": f.Title, "detail": f.Detail})
	}
	return map[string]any{"ok": len(errs) == 0, "failed": failed}
}

// runMigrate runs each repo's migrate (then seed) commands in its worktree with
// the resolved env, so a backend's schema exists before it boots.
func (s *Server) runMigrate(branch string, isMain bool) []doctor.Finding {
	cfg := s.cfg()
	if cfg == nil {
		return nil
	}
	var errs []doctor.Finding
	for repoName, repo := range cfg.Repos {
		cmds := append(append([]string{}, repo.Migrate...), repo.Seed...)
		if len(cmds) == 0 {
			continue
		}
		wt := services.RepoWorktreePath(s.WorkspaceRoot, repoName, branch, isMain)
		env := services.ResolveRepoEnv(s.WorkspaceRoot, cfg, branch, repoName)
		for _, cmd := range cmds {
			argv := shell.Login(cmd)
			if out, err := services.RunTimeoutEnv(10*time.Minute, wt, env, argv[0], argv[1:]...); err != nil {
				errs = append(errs, doctor.Finding{
					ID:           "migrate." + repoName,
					Severity:     doctor.SevError,
					Title:        "migrate failed for " + repoName + ": " + cmd,
					Detail:       lastLines(string(out), 6),
					AgentFixable: true,
				})
				break
			}
		}
	}
	return errs
}

// runSetup runs each repo's setup (install) commands in its worktree so boot
// verification tests a real, installed environment. Failures become findings.
// done tracks repos already installed so a repeated round never reinstalls them.
func (s *Server) runSetup(branch string, isMain bool, done map[string]bool) []doctor.Finding {
	cfg := s.cfg()
	if cfg == nil {
		return nil
	}
	var errs []doctor.Finding
	for repoName, repo := range cfg.Repos {
		if done[repoName] {
			continue
		}
		wt := services.RepoWorktreePath(s.WorkspaceRoot, repoName, branch, isMain)
		// Inject the resolved repo env so install can auth to private package
		// registries (a wired {{secret.*}} credential).
		env := services.ResolveRepoEnv(s.WorkspaceRoot, cfg, branch, repoName)
		ok := true
		for _, cmd := range repo.Setup {
			argv := shell.Login(cmd)
			if out, err := services.RunTimeoutEnv(15*time.Minute, wt, env, argv[0], argv[1:]...); err != nil {
				errs = append(errs, doctor.Finding{
					ID:           "setup." + repoName,
					Severity:     doctor.SevError,
					Title:        "setup failed for " + repoName + ": " + cmd,
					Detail:       lastLines(string(out), 6),
					AgentFixable: true,
				})
				ok = false
				break
			}
		}
		if ok {
			done[repoName] = true
		}
	}
	return errs
}

// verifyBoot starts every service for the branch, waits, and reports any that
// crashed — a clean config only proves the file parses, not that it runs.
func (s *Server) verifyBoot(branch string, isMain bool, grace time.Duration) []doctor.Finding {
	cfg := s.cfg()
	if cfg == nil {
		return nil
	}

	type target struct{ label, window string }
	var targets []target

	for repoName, repo := range cfg.Repos {
		for svcName, svc := range repo.Services {
			if svc == nil || svc.ActiveCmd("") == "" {
				continue
			}
			if ok, _ := s.ServiceControl(serviceRef{Branch: branch, IsMain: isMain, Repo: repoName, Svc: svcName}, "start")["ok"].(bool); !ok {
				continue
			}
			targets = append(targets, target{repoName + "/" + svcName, services.ServiceHolderName(cfg.Session, branch, repoName, svcName)})
		}
	}
	for svcName, svc := range cfg.WsServices {
		if svc == nil || svc.Cmd == "" {
			continue
		}
		if ok, _ := s.ServiceControl(serviceRef{Branch: branch, IsMain: isMain, Svc: svcName}, "start")["ok"].(bool); !ok {
			continue
		}
		targets = append(targets, target{svcName, services.WsServiceHolderName(cfg.Session, branch, svcName)})
	}

	if len(targets) == 0 {
		return nil
	}
	time.Sleep(grace)

	var errs []doctor.Finding
	for _, t := range targets {
		if ptyhost.HolderAlive(t.window) {
			continue
		}
		detail := "exited within " + grace.String() + " of starting"
		if crashed, log := serviceCrash(t.window); crashed && log != "" {
			detail = log
		}
		errs = append(errs, doctor.Finding{
			ID:           "boot." + t.label,
			Severity:     doctor.SevError,
			Title:        "service " + t.label + " failed to boot",
			Detail:       detail,
			AgentFixable: true,
		})
	}
	return errs
}
