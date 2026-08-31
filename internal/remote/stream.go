package remote

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"sync"

	"github.com/pomelohq/pomelo/internal/stream"
)

// errStreamClosed stops a core forward goroutine once the client is gone, so it
// never writes to a ResponseWriter after its handler returned.
var errStreamClosed = errors.New("remote: stream closed")

// AgentInput is the write side of an agent stream (nudge / stop), satisfied by
// the core's ClaudeInput.
type AgentInput interface {
	Send(text string)
	Stop()
}

// AgentStreamer is the core's agent surface the phone taps: subscribe to output
// and nudge an agent. Kept as an interface so remote does not import core.
type AgentStreamer interface {
	OpenAgentStream(sink stream.Sink, done <-chan struct{}, branch string, isMain bool, mode, model, role string) AgentInput
	AgentSend(branch string, isMain bool, mode, model, text string)
}

func (s *Server) SetStreamer(st AgentStreamer) { s.streamer = st }

// sseSink adapts stream.Sink to Server-Sent Events. Agent frames are single-line
// JSON, so each becomes one `data:` event; no WebSocket dependency is pulled in.
type sseSink struct {
	w   http.ResponseWriter
	f   http.Flusher
	ctx context.Context
	mu  sync.Mutex
}

func (s *sseSink) write(b []byte) error {
	// Never touch the ResponseWriter once the client is gone: the handler may
	// have returned and net/http may be tearing the connection down.
	if s.ctx.Err() != nil {
		return errStreamClosed
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, err := fmt.Fprintf(s.w, "data: %s\n\n", b); err != nil {
		return err
	}
	s.f.Flush()
	return nil
}

func (s *sseSink) SendJSON(v any) error       { b, _ := json.Marshal(v); return s.write(b) }
func (s *sseSink) SendJSONBytes(b []byte) error { return s.write(b) }
func (s *sseSink) SendText(b []byte) error     { return s.write(b) }
func (s *sseSink) SendBinary(b []byte) error   { return s.write(b) }
func (s *sseSink) Close() error                { return nil }

func (s *Server) handleAgentStream(w http.ResponseWriter, r *http.Request) {
	if s.streamer == nil {
		http.Error(w, "streaming unavailable", http.StatusServiceUnavailable)
		return
	}
	f, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unsupported", http.StatusInternalServerError)
		return
	}
	q := r.URL.Query()
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.WriteHeader(http.StatusOK)
	f.Flush()

	done := make(chan struct{})
	s.streamer.OpenAgentStream(&sseSink{w: w, f: f, ctx: r.Context()}, done,
		q.Get("branch"), truthy(q.Get("is_main")), q.Get("mode"), q.Get("model"), q.Get("role"))

	<-r.Context().Done()
	close(done)
}

func (s *Server) handleAgentSend(w http.ResponseWriter, r *http.Request) {
	if s.streamer == nil {
		http.Error(w, "streaming unavailable", http.StatusServiceUnavailable)
		return
	}
	var req struct {
		Branch string `json:"branch"`
		IsMain bool   `json:"is_main"`
		Mode   string `json:"mode"`
		Model  string `json:"model"`
		Text   string `json:"text"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	if req.Text == "" {
		http.Error(w, "text required", http.StatusBadRequest)
		return
	}
	s.streamer.AgentSend(req.Branch, req.IsMain, req.Mode, req.Model, req.Text)
	writeJSON(w, map[string]any{"ok": true})
}

// authStream is the GET-capable auth for the SSE stream. A native client can set
// the Authorization header; a token query param is accepted too (TLS-encrypted).
func (s *Server) authStream(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			http.Error(w, "GET only", http.StatusMethodNotAllowed)
			return
		}
		tok := bearer(r)
		if tok == "" {
			tok = r.URL.Query().Get("token")
		}
		if s.token == "" || !tokenEqual(tok, s.token) {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		next(w, r)
	}
}

func truthy(s string) bool { return s == "1" || s == "true" }
