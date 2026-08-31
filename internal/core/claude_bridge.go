package core

import (
	"github.com/pomelohq/pomelo/internal/agent/claude"
	"github.com/pomelohq/pomelo/internal/remote"
	"github.com/pomelohq/pomelo/internal/stream"
)

func (s *Server) ClaudeTerminal(branch string, isMain bool) map[string]any {
	return s.claude.ClaudeTerminal(branch, isMain)
}

func (s *Server) AgentStates() map[string]string { return s.claude.AgentStates() }

type ClaudeInput struct{ d *claude.Driver }

func (c ClaudeInput) Send(text string) { c.d.Send(text) }
func (c ClaudeInput) Stop()            { c.d.Stop() }
func (c ClaudeInput) Cancel(i int)     { c.d.Cancel(i) }

func streamDriver(d *claude.Driver, sink stream.Sink, done <-chan struct{}) ClaudeInput {
	ch := d.Subscribe()
	go func() {
		forwardBytes(sink, ch, done)
		d.Unsubscribe(ch)
	}()
	return ClaudeInput{d: d}
}

// OpenAgentStream and AgentSend satisfy remote.AgentStreamer so a paired phone
// can watch and nudge an agent through the same driver the local app uses.
func (s *Server) OpenAgentStream(sink stream.Sink, done <-chan struct{}, branch string, isMain bool, mode, model, role string) remote.AgentInput {
	return s.OpenClaudeStream(sink, done, branch, isMain, mode, model, role)
}

func (s *Server) AgentSend(branch string, isMain bool, mode, model, text string) {
	s.claude.DriverFor(branch, isMain, mode, model).Send(text)
}

// OpenPTY satisfies remote.PTYStreamer, mirroring an interactive terminal holder
// (e.g. the Claude Code TUI) to a paired phone with its scrollback and live output.
func (s *Server) OpenPTY(sink stream.Sink, done <-chan struct{}, name, wsKey string, cols, rows int) (remote.PTYFeeder, error) {
	return s.OpenPTYStream(sink, name, wsKey, cols, rows, done)
}

func (s *Server) OpenClaudeStream(sink stream.Sink, done <-chan struct{}, branch string, isMain bool, mode, model, role string) ClaudeInput {
	var d *claude.Driver
	switch role {
	case "fixer":
		d = s.claude.FixerDriver(branch, isMain, model)
	case "onboarder":
		d = s.claude.OnboarderDriver(branch, isMain, model)
	default:
		d = s.claude.DriverFor(branch, isMain, mode, model)
	}
	return streamDriver(d, sink, done)
}
