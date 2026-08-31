// Package remote exposes the core dispatch (Query/Command/Fetch) over a LAN
// listener so a paired phone can monitor and nudge a running Pomelo. It is a
// third genuinely-external listener alongside dev-proxy and webhook; the app
// stays "portless" in that its own UI never speaks HTTP. Off by default.
package remote

import (
	"crypto/subtle"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"log"
	"net"
	"net/http"
	"runtime/debug"
	"sync"
	"time"
)

// Dispatcher is the core surface the phone reaches. It is exactly the FFI verb
// set the local SwiftUI app already uses, so remote adds no new business logic.
type Dispatcher interface {
	Query(domain string, params json.RawMessage) any
	Command(domain, action string, params json.RawMessage) any
	Fetch(domain string, params json.RawMessage) []byte
}

// allowedCommands is the remote MVP authorization scope: monitor + (later)
// nudge the agent. Reads (Query/Fetch) are always allowed; mutations are
// default-deny and must be listed here. Service start/stop, git writes, and
// config edits are deliberately absent — a phone on the LAN cannot trigger them.
var allowedCommands = map[string]map[string]bool{
	"pr":        {"refresh": true},
	"workspace": {"create": true, "rename": true},
	"agent":     {"close": true},
}

func commandAllowed(domain, action string) bool {
	return allowedCommands[domain][action]
}

type Server struct {
	disp    Dispatcher
	token   string
	version string
	cert     *tls.Certificate
	fp       string
	streamer AgentStreamer
	pty      PTYStreamer
	feedMu   sync.Mutex
	feeders  map[string]PTYFeeder
	srv      *http.Server
	addr     string
}

func New(disp Dispatcher, token, version string) *Server {
	return &Server{disp: disp, token: token, version: version}
}

// SetTLS makes Start serve HTTPS with this cert; fp is its pinned fingerprint.
func (s *Server) SetTLS(cert tls.Certificate, fp string) {
	s.cert = &cert
	s.fp = fp
}

func (s *Server) Fingerprint() string { return s.fp }

// Start binds addr (e.g. "0.0.0.0:0" for an ephemeral LAN port) and serves in a
// goroutine. It returns the resolved address so the caller can show it / encode
// it into the pairing QR.
func (s *Server) Start(addr string) (string, error) {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return "", fmt.Errorf("remote listen: %w", err)
	}
	s.addr = ln.Addr().String()
	s.srv = &http.Server{
		Handler:           s.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
	}
	if s.cert != nil {
		cfg, err := s.tlsConfig()
		if err != nil {
			_ = ln.Close()
			return "", err
		}
		s.srv.TLSConfig = cfg
		go func() { _ = s.srv.ServeTLS(ln, "", "") }()
	} else {
		go func() { _ = s.srv.Serve(ln) }()
	}
	return s.addr, nil
}

func (s *Server) Stop() {
	if s.srv != nil {
		_ = s.srv.Close()
	}
}

func (s *Server) Addr() string { return s.addr }

func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	// Unauthenticated liveness for discovery — carries no session data.
	mux.HandleFunc("/ping", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, map[string]any{"pomelo": true, "version": s.version})
	})
	mux.HandleFunc("/rpc/query", recoverHTTP(s.auth(s.handleQuery)))
	mux.HandleFunc("/rpc/command", recoverHTTP(s.auth(s.handleCommand)))
	mux.HandleFunc("/rpc/fetch", recoverHTTP(s.auth(s.handleFetch)))
	mux.HandleFunc("/rpc/stream/agent", recoverHTTP(s.authStream(s.handleAgentStream)))
	mux.HandleFunc("/rpc/agent/send", recoverHTTP(s.auth(s.handleAgentSend)))
	mux.HandleFunc("/rpc/stream/pty", recoverHTTP(s.authStream(s.handlePTYStream)))
	mux.HandleFunc("/rpc/pty/input", recoverHTTP(s.auth(s.handlePTYInput)))
	mux.HandleFunc("/rpc/pty/resize", recoverHTTP(s.auth(s.handlePTYResize)))
	return mux
}

// recoverHTTP guarantees a remote request can never panic the whole process:
// the app hosts this listener in-process, so an unhandled panic in a handler
// would take down the user's live session.
func recoverHTTP(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		defer func() {
			if v := recover(); v != nil {
				log.Printf("remote: recovered panic on %s: %v\n%s", r.URL.Path, v, debug.Stack())
			}
		}()
		next(w, r)
	}
}

func (s *Server) auth(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "POST only", http.StatusMethodNotAllowed)
			return
		}
		if s.token == "" || !tokenEqual(bearer(r), s.token) {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		next(w, r)
	}
}

type rpcReq struct {
	Domain string          `json:"domain"`
	Action string          `json:"action"`
	Params json.RawMessage `json:"params"`
}

func (s *Server) handleQuery(w http.ResponseWriter, r *http.Request) {
	req, ok := decodeReq(w, r)
	if !ok {
		return
	}
	writeJSON(w, s.disp.Query(req.Domain, req.Params))
}

func (s *Server) handleCommand(w http.ResponseWriter, r *http.Request) {
	req, ok := decodeReq(w, r)
	if !ok {
		return
	}
	if !commandAllowed(req.Domain, req.Action) {
		http.Error(w, "command not allowed over remote", http.StatusForbidden)
		return
	}
	writeJSON(w, s.disp.Command(req.Domain, req.Action, req.Params))
}

func (s *Server) handleFetch(w http.ResponseWriter, r *http.Request) {
	req, ok := decodeReq(w, r)
	if !ok {
		return
	}
	w.Header().Set("Content-Type", "application/octet-stream")
	_, _ = w.Write(s.disp.Fetch(req.Domain, req.Params))
}

func decodeReq(w http.ResponseWriter, r *http.Request) (rpcReq, bool) {
	var req rpcReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return rpcReq{}, false
	}
	if req.Params == nil {
		req.Params = json.RawMessage("{}")
	}
	return req, true
}

func bearer(r *http.Request) string {
	const p = "Bearer "
	h := r.Header.Get("Authorization")
	if len(h) > len(p) && h[:len(p)] == p {
		return h[len(p):]
	}
	return ""
}

func tokenEqual(a, b string) bool {
	return subtle.ConstantTimeCompare([]byte(a), []byte(b)) == 1
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}
