package core

import (
	"encoding/json"
	"fmt"
	"io"
	"time"

	"github.com/pomelohq/pomelo/internal/agent/claude"
	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/doctor"
	"github.com/pomelohq/pomelo/internal/stream"
)

func (s *Server) OpenOnboardStream(sink stream.Sink, done <-chan struct{}, branch string, isMain bool, model string) ClaudeInput {
	d := s.claude.OnboarderDriver(branch, isMain, model)
	return streamDriver(d, sink, done)
}

func (s *Server) RunOnboardCLI(branch string, isMain bool, model string, out io.Writer) error {
	d := s.claude.OnboarderDriver(branch, isMain, model)
	ch := d.Subscribe()
	defer d.Unsubscribe(ch)
	go printEvents(ch, out)

	const maxRounds = 6
	d.Enqueue(claude.OnboardFirstTurn())
	setupDone := map[string]bool{}
	for round := 1; ; round++ {
		if err := waitIdle(d, 20*time.Minute); err != nil {
			return err
		}
		s.reloadConfig() // the agent rewrote pom.yml via MCP; verify against its edits, not the seed
		errs := actionableFindings(s.cfg(), s.WorkspaceRoot, s.Project)
		if len(errs) == 0 {
			fmt.Fprintln(out, "\nInstalling deps (setup)")
			errs = s.runSetup(branch, isMain, setupDone)
		}
		if len(errs) == 0 {
			fmt.Fprintln(out, "Deps installed - booting services to verify")
			errs = s.verifyBoot(branch, isMain, 12*time.Second)
			if len(errs) == 0 {
				fmt.Fprintln(out, "Config clean + deps installed + services booted - project is runnable.")
				return nil
			}
		}
		if round >= maxRounds {
			summarizeOnboard(s.cfg(), errs).render(out)
			return nil
		}
		fmt.Fprintf(out, "\n%d finding(s) remain - nudging agent (round %d)\n", len(errs), round+1)
		d.Enqueue(nudgeTurn(errs))
	}
}

func (s *Server) reloadConfig() {
	p, err := config.FindConfigFrom(s.WorkspaceRoot)
	if err != nil {
		return
	}
	if cfg, err := config.Load(p); err == nil {
		s.setCfg(cfg)
	}
}

func actionableFindings(cfg *config.Config, root, session string) []doctor.Finding {
	var errs []doctor.Finding
	for _, f := range doctor.Diagnose(cfg, root, session) {
		if f.Severity == doctor.SevError || (f.Severity == doctor.SevWarn && f.AgentFixable) {
			errs = append(errs, f)
		}
	}
	return errs
}

func nudgeTurn(errs []doctor.Finding) string {
	msg := "config_doctor still reports errors — fix them and re-check:\n"
	for _, f := range errs {
		msg += "- " + f.Title
		if f.Detail != "" {
			msg += ": " + f.Detail
		}
		msg += "\n"
	}
	return msg
}

func waitIdle(d *claude.Driver, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	time.Sleep(500 * time.Millisecond)
	for {
		if !d.Busy() {
			return nil
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("onboarding timed out after %s", timeout)
		}
		time.Sleep(500 * time.Millisecond)
	}
}

func printEvents(ch chan []byte, out io.Writer) {
	for b := range ch {
		var ev claude.StreamEvent
		if json.Unmarshal(b, &ev) != nil {
			continue
		}
		switch ev.Kind {
		case "text":
			fmt.Fprint(out, ev.Text)
		case "tool_use":
			fmt.Fprintf(out, "\n> %s %s\n", ev.Tool, string(ev.Input))
		case "error":
			fmt.Fprintf(out, "\n[error] %s\n", ev.Err)
		case "system":
			fmt.Fprintf(out, "\n[%s]\n", ev.Text)
		}
	}
}
