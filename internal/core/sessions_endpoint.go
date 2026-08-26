package core

import (
	"encoding/json"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/services"
	"github.com/pomelohq/pomelo/internal/sessions"
)

func (s *Server) handleSessionList(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.SessionList())
}

func (s *Server) SessionList() map[string]any {
	reg := sessions.Load()
	type item struct {
		Name      string `json:"name"`
		Path      string `json:"path"`
		Current   bool   `json:"current"`
		Running   bool   `json:"running"`
		Available bool   `json:"available"`
	}
	out := make([]item, 0, len(reg.Sessions))
	for _, ss := range reg.Sessions {
		_, ok := configFileIn(ss.Path)
		out = append(out, item{
			Name: ss.Name, Path: ss.Path, Current: ss.Name == s.Project,
			Running: services.SessionRunning(ss.Name), Available: ok,
		})
	}
	return map[string]any{
		"sessions": out,
		"current":  s.Project,
		"active":   s.Project,
		"root":     sessions.SessionsRoot(),
	}
}

func (s *Server) handleSessionDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	var req struct {
		Name  string `json:"name"`
		Purge bool   `json:"purge"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	writeJSON(w, s.SessionDelete(req.Name, req.Purge))
}

func (s *Server) SessionDelete(name string, purge bool) map[string]any {
	reg := sessions.Load()
	ss := reg.Get(name)
	if ss == nil {
		return map[string]any{"ok": false, "error": "unknown session"}
	}
	if name == reg.Current || ss.Path == s.WorkspaceRoot {
		return map[string]any{"ok": false, "error": "can't delete the active session — switch to another first"}
	}

	// Tear the session's runtime down before removing it: kill its ptyhost holders
	// (services + shells) and bring its shared-service stack down so no containers
	// (or, on purge, volumes) are orphaned. Best-effort — proceed regardless.
	services.StopSession(name)
	compose := filepath.Join(ss.Path, "docker-compose.shared.yml")
	if _, err := os.Stat(compose); err == nil {
		args := []string{"compose", "-f", compose, "-p", name + "-shared", "down"}
		if purge {
			args = append(args, "-v")
		}
		_ = exec.Command("docker", args...).Run()
	}

	if purge {
		clean := filepath.Clean(ss.Path)
		if !filepath.IsAbs(clean) || clean == "/" || clean == filepath.Dir(clean) {
			return map[string]any{"ok": false, "error": "refusing to delete unsafe path: " + clean}
		}
		if _, ok := configFileIn(clean); !ok {
			return map[string]any{"ok": false, "error": "not a session directory (no pom.yml): " + clean}
		}
		if err := os.RemoveAll(clean); err != nil {
			return map[string]any{"ok": false, "error": err.Error()}
		}
	}
	reg.Remove(name)
	_ = reg.Save()
	return map[string]any{"ok": true}
}

func (s *Server) handleRefresh(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.cfg() == nil {
		http.Error(w, "no project config", http.StatusServiceUnavailable)
		return
	}
	services.StopSession(s.cfg().Session)
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"ok": true})
}

func (s *Server) handleSessionReleaseSlot(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	var req struct {
		Name string `json:"name"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	if req.Name == "" {
		http.Error(w, "name required", http.StatusBadRequest)
		return
	}
	stopped := services.StopSession(req.Name) > 0
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "stopped": stopped})
}

func (s *Server) handleSessionSwitch(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	var req struct {
		Name string `json:"name"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	writeJSON(w, s.SessionSwitch(req.Name))
}

func (s *Server) SessionSwitch(name string) map[string]any {
	reg := sessions.Load()
	ss := reg.Get(name)
	if ss == nil {
		return map[string]any{"ok": false, "error": "unknown session"}
	}
	cfgPath, ok := configFileIn(ss.Path)
	if !ok {
		return map[string]any{"ok": false, "error": "no pom.yml in " + ss.Path}
	}
	cfg, err := config.Load(cfgPath)
	if err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	s.setCfg(cfg)
	s.WorkspaceRoot = ss.Path
	s.Project = cfg.Session
	s.DefaultBranch = cfg.GlobalDefaultBranch()
	s.claude.ResetForSwitch(ss.Path)
	services.InitNetwork(ss.Path, cfg.Session, cfg)
	reg.Touch(name, ss.Path, time.Now().Unix())
	_ = reg.Save()
	return map[string]any{"ok": true, "project": cfg.Session}
}
