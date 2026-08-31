package remote

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"sync"

	"github.com/pomelohq/pomelo/internal/stream"
)

// PTYFeeder writes input (keystrokes, resize control) back into a terminal,
// satisfied by the core's PtyInput.
type PTYFeeder interface{ Feed(data []byte) }

// PTYStreamer mirrors an interactive terminal holder (the Claude Code TUI the
// desktop shows) to the phone: snapshot scrollback plus live output.
type PTYStreamer interface {
	OpenPTY(sink stream.Sink, done <-chan struct{}, name, wsKey string, cols, rows int) (PTYFeeder, error)
}

func (s *Server) SetPTYStreamer(p PTYStreamer) { s.pty = p }

// ptySSESink frames terminal traffic for SSE: binary output is base64 behind a
// "b" tag, control JSON (size/exit) behind a "c" tag, so the phone can tell the
// xterm byte stream from control frames.
type ptySSESink struct {
	w   http.ResponseWriter
	f   http.Flusher
	ctx context.Context
	mu  sync.Mutex
}

func (s *ptySSESink) emit(tag byte, payload string) error {
	if s.ctx.Err() != nil {
		return errStreamClosed
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, err := fmt.Fprintf(s.w, "data: %c%s\n\n", tag, payload); err != nil {
		return err
	}
	s.f.Flush()
	return nil
}

func (s *ptySSESink) SendBinary(b []byte) error   { return s.emit('b', base64.StdEncoding.EncodeToString(b)) }
func (s *ptySSESink) SendText(b []byte) error     { return s.emit('c', string(b)) }
func (s *ptySSESink) SendJSONBytes(b []byte) error { return s.emit('c', string(b)) }
func (s *ptySSESink) SendJSON(v any) error        { b, _ := json.Marshal(v); return s.emit('c', string(b)) }
func (s *ptySSESink) Close() error                { return nil }

func (s *Server) handlePTYStream(w http.ResponseWriter, r *http.Request) {
	if s.pty == nil {
		http.Error(w, "pty unavailable", http.StatusServiceUnavailable)
		return
	}
	f, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unsupported", http.StatusInternalServerError)
		return
	}
	q := r.URL.Query()
	window := q.Get("window")
	if window == "" {
		http.Error(w, "window required", http.StatusBadRequest)
		return
	}
	cols, _ := strconv.Atoi(q.Get("cols"))
	rows, _ := strconv.Atoi(q.Get("rows"))

	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.WriteHeader(http.StatusOK)
	f.Flush()

	done := make(chan struct{})
	feeder, err := s.pty.OpenPTY(&ptySSESink{w: w, f: f, ctx: r.Context()}, done, window, q.Get("ws_key"), cols, rows)
	if err != nil {
		close(done)
		return
	}
	s.setFeeder(window, feeder)
	defer s.clearFeeder(window)

	<-r.Context().Done()
	close(done)
}

// handlePTYInput forwards keystrokes (including keys the iOS keyboard lacks:
// esc/tab/ctrl/arrows) to the agent. It deliberately never resizes the shared
// holder — resize is the geometry change that corrupts the Mac's live session,
// so no resize endpoint exists.
func (s *Server) handlePTYInput(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Window  string `json:"window"`
		DataB64 string `json:"data_b64"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	feeder := s.feeder(req.Window)
	if feeder == nil {
		http.Error(w, "no active terminal", http.StatusNotFound)
		return
	}
	data, err := base64.StdEncoding.DecodeString(req.DataB64)
	if err != nil {
		http.Error(w, "bad base64", http.StatusBadRequest)
		return
	}
	feeder.Feed(data)
	writeJSON(w, map[string]any{"ok": true})
}

// handlePTYResize matches the shared holder to the phone's terminal size so a
// full-screen TUI (Claude Code) renders correctly on the phone. Shared holders
// take the smallest attached client's size, tmux-style.
func (s *Server) handlePTYResize(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Window string `json:"window"`
		Cols   int    `json:"cols"`
		Rows   int    `json:"rows"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	feeder := s.feeder(req.Window)
	if feeder == nil || req.Cols <= 0 || req.Rows <= 0 {
		http.Error(w, "no active terminal", http.StatusNotFound)
		return
	}
	feeder.Feed([]byte(fmt.Sprintf(`{"__pom":"resize","cols":%d,"rows":%d}`, req.Cols, req.Rows)))
	writeJSON(w, map[string]any{"ok": true})
}

func (s *Server) setFeeder(window string, f PTYFeeder) {
	s.feedMu.Lock()
	if s.feeders == nil {
		s.feeders = map[string]PTYFeeder{}
	}
	s.feeders[window] = f
	s.feedMu.Unlock()
}

func (s *Server) clearFeeder(window string) {
	s.feedMu.Lock()
	delete(s.feeders, window)
	s.feedMu.Unlock()
}

func (s *Server) feeder(window string) PTYFeeder {
	s.feedMu.Lock()
	defer s.feedMu.Unlock()
	return s.feeders[window]
}
