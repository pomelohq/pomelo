package core

import (
	"time"

	"github.com/pomelohq/pomelo/internal/doctor"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

// verifyBoot starts every configured service for the branch, waits a grace
// period, and reports any that crashed. config_doctor being clean only proves the
// file parses — the moat is that the environment actually runs, so onboarding is
// not "done" until services boot without crashing.
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
