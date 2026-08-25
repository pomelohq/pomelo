package core

import (
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

func (s *Server) handleServiceStart(w http.ResponseWriter, r *http.Request) {
	s.serviceAction(w, r, "start")
}
func (s *Server) handleServiceStop(w http.ResponseWriter, r *http.Request) {
	s.serviceAction(w, r, "stop")
}
func (s *Server) handleServiceRestart(w http.ResponseWriter, r *http.Request) {
	s.serviceAction(w, r, "restart")
}

func (s *Server) handleServiceMode(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.cfg() == nil {
		http.Error(w, "no project config", http.StatusServiceUnavailable)
		return
	}
	var ref serviceRef
	if err := json.NewDecoder(r.Body).Decode(&ref); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	writeJSON(w, s.ServiceMode(ref.Repo, ref.Svc, ref.Mode))
}

func (s *Server) ServiceMode(repo, svc, mode string) map[string]any {
	if s.cfg() == nil {
		return map[string]any{"ok": false, "error": "no project config"}
	}
	cfgRepo, ok := s.cfg().Repos[repo]
	if !ok {
		return map[string]any{"ok": false, "error": "unknown repo"}
	}
	sc, ok := cfgRepo.Services[svc]
	if !ok || len(sc.Modes) == 0 {
		return map[string]any{"ok": false, "error": "service has no modes"}
	}
	s.modeMu.Lock()
	s.modeOverrides[repo+"~"+svc] = mode
	s.modeMu.Unlock()
	return map[string]any{"ok": true, "mode": mode}
}

func (s *Server) svcMode(repo, svcName string, svc *config.Service) string {
	s.modeMu.Lock()
	mode := s.modeOverrides[repo+"~"+svcName]
	s.modeMu.Unlock()
	if mode == "" {
		mode = svc.Mode
	}
	return mode
}

type openBrowserReq struct {
	Branch string `json:"branch"`
	IsMain bool   `json:"is_main"`
	Repo   string `json:"repo"`
	Svc    string `json:"svc"`
}

func (s *Server) handleServiceURL(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.cfg() == nil {
		http.Error(w, "no project config", http.StatusServiceUnavailable)
		return
	}
	var req openBrowserReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	writeJSON(w, s.ServiceURL(req.Branch, req.Repo, req.Svc))
}

func (s *Server) ServiceURL(branch, repo, svc string) map[string]any {
	if s.cfg() == nil {
		return map[string]any{"error": "no project config"}
	}
	cfgRepo, ok := s.cfg().Repos[repo]
	if !ok {
		return map[string]any{"error": "unknown repo"}
	}
	sc, ok := cfgRepo.Services[svc]
	if !ok || !sc.HasPort() {
		return map[string]any{"error": "service has no port"}
	}
	alias := cfgRepo.Alias
	if alias == "" {
		alias = repo
	}
	wsKey := services.PortWsKey(branch)
	port := services.Port(s.WorkspaceRoot, wsKey, alias+"~"+svc)
	url := fmt.Sprintf("http://%s:%d", services.BindIP(), port)
	if u := s.devProxyURLFor(s.cfg(), branch, alias, svc); u != "" {
		url = u
	}
	return map[string]any{"url": url}
}

func (s *Server) serviceAction(w http.ResponseWriter, r *http.Request, action string) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	var ref serviceRef
	if err := json.NewDecoder(r.Body).Decode(&ref); err != nil {
		http.Error(w, "bad json: "+err.Error(), http.StatusBadRequest)
		return
	}
	writeJSON(w, s.ServiceControl(ref, action))
}

func (s *Server) ServiceControlJSON(refJSON, action string) map[string]any {
	var ref serviceRef
	if err := json.Unmarshal([]byte(refJSON), &ref); err != nil {
		return map[string]any{"ok": false, "error": "bad json"}
	}
	return s.ServiceControl(ref, action)
}

func (s *Server) ServiceControl(ref serviceRef, action string) map[string]any {
	if s.cfg() == nil {
		return map[string]any{"ok": false, "error": "no project config loaded"}
	}
	session := s.cfg().Session
	var window string
	var startFn func() error

	if ref.isWS() {
		svc, ok := s.cfg().WsServices[ref.Svc]
		if !ok || svc == nil || svc.Cmd == "" {
			return map[string]any{"ok": false, "error": "unknown workspace-level service"}
		}
		window = services.WsServiceHolderName(s.cfg().Session, ref.Branch, ref.Svc)
		startFn = func() error { return s.startWsWindow(window, svc.Cmd, ref) }
	} else {
		cfgRepo, ok := s.cfg().Repos[ref.Repo]
		if !ok {
			return map[string]any{"ok": false, "error": "unknown repo: " + ref.Repo}
		}
		svc, ok := cfgRepo.Services[ref.Svc]
		if !ok || svc.ActiveCmd("") == "" {
			return map[string]any{"ok": false, "error": "unknown / blank service"}
		}
		window = services.ServiceHolderName(s.cfg().Session, ref.Branch, ref.Repo, ref.Svc)
		startFn = func() error { return s.startWindow(session, window, cfgRepo, svc, ref) }
	}

	isRunning := func() bool { return ptyhost.HolderAlive(window) }
	stop := func() { _ = ptyhost.KillHolder(window) }

	switch action {
	case "stop":
		stop()
		pp := pomAllocatedPorts()
		go ptyhost.ReapOrphanServices(s.WorkspaceRoot, pp)
	case "restart":
		stop()
		ptyhost.ReapOrphanServices(s.WorkspaceRoot, pomAllocatedPorts())
		if err := startFn(); err != nil {
			return map[string]any{"ok": false, "error": err.Error()}
		}
	case "start":
		if isRunning() {
			break
		}
		s.ensureSharedServices()
		if !ref.isWS() && len(s.cfg().SharedServices) > 0 {
			services.EnsureWorkspaceDatabases(s.cfg(), ref.Branch)
		}
		if err := startFn(); err != nil {
			return map[string]any{"ok": false, "error": err.Error()}
		}
	}

	return map[string]any{"ok": true, "window": window, "action": action}
}

func (s *Server) doServiceRestart(ref serviceRef) error {
	cfg := s.cfg()
	if cfg == nil {
		return fmt.Errorf("no config")
	}
	session := cfg.Session
	cfgRepo, ok := cfg.Repos[ref.Repo]
	if !ok {
		return fmt.Errorf("unknown repo %s", ref.Repo)
	}
	svc, ok := cfgRepo.Services[ref.Svc]
	if !ok || svc == nil {
		return fmt.Errorf("unknown service %s", ref.Svc)
	}
	alias := cfgRepo.Alias
	if alias == "" {
		alias = ref.Repo
	}
	_ = alias
	holder := services.ServiceHolderName(cfg.Session, ref.Branch, ref.Repo, ref.Svc)
	_ = ptyhost.KillHolder(holder)
	return s.startWindow(session, holder, cfgRepo, svc, ref)
}
