package core

import (
	"bytes"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/services"
)

const webhookBodyCap = 32 << 20

func (s *Server) startWebhookRelay() {
	cfg := s.cfg()
	if cfg == nil || len(cfg.Repos) == 0 {
		return
	}
	port := s.webPort() + 1
	addr := fmt.Sprintf("127.0.0.1:%d", port)
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		log.Printf("webhook relay: %s already in use — skipping (another pom owns it)", addr)
		return
	}
	srv := &http.Server{Handler: http.HandlerFunc(s.handleWebhookRelay), ReadHeaderTimeout: 5 * time.Second}
	log.Printf("webhook relay on %s", addr)
	if err := srv.Serve(ln); err != nil && err != http.ErrServerClosed {
		log.Printf("webhook relay stopped: %v", err)
	}
}

func (s *Server) handleWebhookRelay(w http.ResponseWriter, r *http.Request) {
	cfg := s.cfg()
	if cfg == nil || len(cfg.Repos) == 0 {
		http.Error(w, "relay not configured", http.StatusServiceUnavailable)
		return
	}

	parts := strings.SplitN(strings.TrimPrefix(r.URL.Path, "/"), "/", 3)
	if len(parts) >= 2 && parts[0] != "" && parts[1] != "" {
		target := parts[0] + "/" + parts[1]
		if svcKey, ok := resolveSvcKey(cfg, target); ok {
			fwd := "/"
			if len(parts) == 3 {
				fwd = "/" + parts[2]
			}
			s.fanOutWebhook(w, r, svcKey, target, fwd)
			return
		}
	}
	http.Error(w, "no webhook route for "+r.URL.Path+" (use /<repo>/<service>)", http.StatusNotFound)
}

func (s *Server) fanOutWebhook(w http.ResponseWriter, r *http.Request, svcKey, target, fwd string) {
	body, _ := io.ReadAll(io.LimitReader(r.Body, webhookBodyCap))
	_ = r.Body.Close()
	ports := s.svcPortsListening(svcKey)
	w.Header().Set("Content-Type", "application/json")
	fmt.Fprintf(w, `{"ok":true,"service":%q,"fanout":%d}`, target, len(ports))
	hdr := r.Header.Clone()
	method, rawQuery := r.Method, r.URL.RawQuery
	go func() {
		for _, p := range ports {
			forwardWebhook(method, p, fwd, rawQuery, hdr, body)
		}
	}()
}

func (s *Server) branchForHostLabel(label string) string {
	s.bcMu.Lock()
	defer s.bcMu.Unlock()
	if s.bcMap == nil || time.Since(s.bcAt) > 5*time.Second {
		known := map[string]bool{}
		if s.cfg() != nil {
			for n := range s.cfg().Repos {
				known[n] = true
			}
		}
		wss := scanWorkspaces(s.WorkspaceRoot, s.DefaultBranch, known, true)
		short := map[string]int{}
		for _, ws := range wss {
			short[services.WorkspaceLabel(ws.Branch)]++
		}
		m := map[string]string{}
		for _, ws := range wss {
			m[services.BranchHost(ws.Branch)] = ws.Branch
			if wl := services.WorkspaceLabel(ws.Branch); short[wl] == 1 {
				m[wl] = ws.Branch
			}
		}
		s.bcMap, s.bcAt = m, time.Now()
	}
	return s.bcMap[label]
}

func forwardWebhook(method string, port int, path, rawQuery string, hdr http.Header, body []byte) {
	url := fmt.Sprintf("http://%s:%d%s", services.BindIP(), port, path)
	if rawQuery != "" {
		url += "?" + rawQuery
	}
	req, err := http.NewRequest(method, url, bytes.NewReader(body))
	if err != nil {
		return
	}
	for k, vs := range hdr {
		if strings.EqualFold(k, "Host") {
			continue
		}
		for _, v := range vs {
			req.Header.Add(k, v)
		}
	}
	resp, err := (&http.Client{Timeout: 20 * time.Second}).Do(req)
	if err != nil {
		log.Printf("webhook fanout → :%d %s: %v", port, path, err)
		return
	}
	_ = resp.Body.Close()
}

func (s *Server) svcPortsListening(svcKey string) []int {
	var ports []int
	for _, p := range services.PortMgr().SvcPorts(svcKey) {
		if p > 0 && portListening(p) {
			ports = append(ports, p)
		}
	}
	return ports
}

func (s *Server) webPort() int {
	if i := strings.LastIndex(s.Addr, ":"); i >= 0 {
		if p, err := strconv.Atoi(s.Addr[i+1:]); err == nil {
			return p
		}
	}
	// Lets a dev build run its dev-proxy/webhook on a different port than an
	// installed instance, so the two can coexist (paired with a separate state dir).
	if v := os.Getenv("POM_WEB_PORT"); v != "" {
		if p, err := strconv.Atoi(v); err == nil {
			return p
		}
	}
	return 8765
}

func portListening(port int) bool {
	c, err := net.DialTimeout("tcp", fmt.Sprintf("%s:%d", services.BindIP(), port), 300*time.Millisecond)
	if err != nil {
		return false
	}
	_ = c.Close()
	return true
}

func resolveSvcKey(cfg *config.Config, target string) (string, bool) {
	alias, svc := target, ""
	if i := strings.IndexByte(target, '/'); i >= 0 {
		alias, svc = target[:i], target[i+1:]
	}
	var dir *config.Dir
	ralias := ""
	for name, d := range cfg.Repos {
		a := d.Alias
		if a == "" {
			a = name
		}
		if a == alias || name == alias {
			dir, ralias = d, a
			break
		}
	}
	if dir == nil {
		return "", false
	}
	if svc == "" {
		if len(dir.Services) != 1 {
			return "", false
		}
		for k := range dir.Services {
			svc = k
		}
	}
	return ralias + "~" + svc, true
}
