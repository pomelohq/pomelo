package core

import (
	"encoding/json"
	"fmt"
	"github.com/pomelohq/pomelo/internal/provider/shell"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/shirou/gopsutil/v4/process"
	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

type serviceRef struct {
	Branch       string `json:"branch"`
	IsMain       bool   `json:"is_main"`
	Repo         string `json:"repo"`
	Svc          string `json:"svc"`
	Mode         string `json:"mode,omitempty"`
	Relocate     bool   `json:"relocate,omitempty"`
	AutoRelocate *bool  `json:"auto_relocate,omitempty"`
}

func (r serviceRef) isWS() bool { return r.Repo == "" || r.Repo == "_ws" }

func shellHolderName(branch string, isMain bool, label string) string {
	return "sh-" + services.WsKey(branch, isMain) + "-" + label + "-" +
		strconv.FormatInt(time.Now().UnixNano()%100000, 10)
}

func (s *Server) handlePaneKill(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	var req struct {
		PaneID string `json:"pane_id"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	writeJSON(w, s.PaneKill(req.PaneID))
}

func (s *Server) handlePaneBusy(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]any{"busy": s.PaneBusy(r.URL.Query().Get("holder"))})
}

func (s *Server) PaneBusy(holder string) bool {
	name := strings.TrimPrefix(holder, "pty:")
	busy := false
	if pid := ptyhost.HolderPID(name); pid > 0 {
		if p, err := process.NewProcess(int32(pid)); err == nil {
			if kids, err := p.Children(); err == nil {
				for _, k := range kids {
					if gk, _ := k.Children(); len(gk) > 0 {
						busy = true
						break
					}
				}
			}
		}
	}
	return busy
}

func (s *Server) handlePtyReap(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.PtyReap())
}

func (s *Server) PtyReap() map[string]any { return map[string]any{"reaped": reapEphemeralShells()} }

func (s *Server) PaneKill(paneID string) map[string]any {
	if paneID == "" {
		return map[string]any{"ok": false, "error": "pane_id required"}
	}
	if err := ptyhost.KillHolder(strings.TrimPrefix(paneID, "pty:")); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	return map[string]any{"ok": true}
}

func reapEphemeralShells() int {
	var names []string
	for _, h := range ptyhost.Holders() {
		if services.HolderFor(h.Name).Reapable() {
			names = append(names, h.Name)
		}
	}
	ptyhost.KillHoldersNow(names)
	return len(names)
}

func (s *Server) restartStaleServices() []string {
	cfg := s.cfg()
	if cfg == nil || s.WorkspaceRoot == "" {
		return nil
	}
	known := map[string]bool{}
	for n := range cfg.Repos {
		known[n] = true
	}
	var restarted []string
	for _, ws := range scanWorkspaces(s.WorkspaceRoot, s.DefaultBranch, known, true) {
		wsKey := services.PortWsKey(ws.Branch)
		for _, repo := range ws.Repos {
			cfgRepo, ok := cfg.Repos[repo.Name]
			if !ok {
				continue
			}
			alias := cfgRepo.Alias
			if alias == "" {
				alias = repo.Name
			}
			for _, svcName := range cfgRepo.ServiceOrder {
				svc := cfgRepo.Services[svcName]
				if svc == nil || !svc.HasPort() {
					continue
				}
				if !services.ServiceRunning(cfg.Session, ws.Branch, repo.Name, svcName) {
					continue
				}
				port := services.Port(s.WorkspaceRoot, wsKey, alias+"~"+svcName)
				if port <= 0 || !services.IsPortFree(port) {
					continue
				}
				if !services.PortEverBound(wsKey, alias+"~"+svcName) {
					continue
				}
				ref := serviceRef{Branch: ws.Branch, IsMain: ws.IsMain, Repo: repo.Name, Svc: svcName}
				if err := s.doServiceRestart(ref); err == nil {
					restarted = append(restarted, ws.Branch+"/"+alias+"/"+svcName)
				}
			}
		}
	}
	return restarted
}

func (s *Server) startWindow(session, window string, cfgRepo *config.Dir, svc *config.Service, ref serviceRef) error {
	configDir := s.WorkspaceRoot
	if configDir == "" {
		return fmt.Errorf("workspace root unknown")
	}

	wtPath := repoWorktreePath(configDir, ref.Repo, ref.Branch, ref.IsMain)
	alias := cfgRepo.Alias
	if alias == "" {
		alias = ref.Repo
	}
	wsKey := services.PortWsKey(ref.Branch)
	port := 0
	if svc.HasPort() {
		svcKey := alias + "~" + ref.Svc
		if ref.Relocate {
			services.ReleaseServicePort(wsKey, svcKey)
		}
		var err error
		if port, err = services.PreflightPort(configDir, wsKey, svcKey); err != nil {
			return err
		}
		autoReloc := ref.AutoRelocate == nil || *ref.AutoRelocate
		if port > 0 && !services.IsPortFree(port) {
			if ref.Relocate || autoReloc {
				services.ReleaseServicePort(wsKey, svcKey)
				if port, err = services.PreflightPort(configDir, wsKey, svcKey); err != nil {
					return err
				}
			}
			if !services.IsPortFree(port) {
				if who := services.WhoListensPort(port); who != "" {
					return fmt.Errorf("port %d is already held by %s — stop it if it's not this service, or use a new port", port, who)
				}
				return fmt.Errorf("port %d is already in use — free it or use a new port", port)
			}
		}
	}
	cmd := services.BuildServiceCmd(wtPath, cfgRepo, svc, port, s.svcMode(ref.Repo, ref.Svc, svc))
	services.RegenerateWorkspaceEnv(configDir, s.cfg(), ref.Branch)
	env := services.ResolveServiceEnv(configDir, s.cfg(), ref.Branch, ref.Repo, ref.Svc)
	return services.SpawnHolderEnv(window, wtPath, 0, 0, shell.Login(cmd), env)
}

func repoWorktreePath(root, repo, branch string, isMain bool) string {
	return services.RepoWorktreePath(root, repo, branch, isMain)
}

func dirExists(path string) bool {
	st, err := osStat(path)
	return err == nil && st.IsDir()
}

func (s *Server) workspaceRoot(branch string, isMain bool) string {
	return services.WorkspaceRootDir(s.WorkspaceRoot, branch, isMain)
}
