package core

import (
	"time"

	"github.com/pomelohq/pomelo/internal/doctor"
	"github.com/pomelohq/pomelo/internal/provider/shell"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

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
		ok := true
		for _, cmd := range repo.Setup {
			argv := shell.Login(cmd)
			if out, err := services.RunTimeout(15*time.Minute, wt, argv[0], argv[1:]...); err != nil {
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
