package core

import (
	"bytes"
	"encoding/json"
	"log"
	"net/http"
	"net/http/httputil"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"sync/atomic"
	"time"

	"github.com/pomelohq/pomelo/internal/agent/claude"
	"github.com/pomelohq/pomelo/internal/commands"
	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/lock"
	"github.com/pomelohq/pomelo/internal/plugin"
	"github.com/pomelohq/pomelo/internal/provider/forge"
	"github.com/pomelohq/pomelo/internal/provider/tracker"
	"github.com/pomelohq/pomelo/internal/services"
)

var Version = "dev"

type Server struct {
	Addr          string
	Project       string
	WorkspaceRoot string
	DefaultBranch string

	cfgv atomic.Pointer[config.Config]

	modeMu        sync.Mutex
	modeOverrides map[string]string

	proxyLog proxyLog

	dpOnce sync.Once
	dp     *httputil.ReverseProxy
	epOnce sync.Once
	ep     *httputil.ReverseProxy

	ppMu    sync.Mutex
	ppCache map[string]proxyPortEntry

	bcMu  sync.Mutex
	bcMap map[string]string
	bcAt  time.Time

	claude   *claude.Feature
	jira     tracker.Provider
	pr       *forge.Feature
	activity *activityFeature
	git      *gitFeature

	resMu   sync.Mutex
	resStat commands.ResourceStat
}

func New(addr, project, workspaceRoot, defaultBranch string, cfg *config.Config) *Server {
	s := &Server{
		Addr: addr, Project: project, WorkspaceRoot: workspaceRoot,
		DefaultBranch: defaultBranch,
		modeOverrides: map[string]string{},
	}
	if cfg != nil {
		s.cfgv.Store(cfg)
		services.SetSharedStable(cfg.Session)
	}
	s.claude = claude.New(s.cfg, workspaceRoot, s.mcpConfigJSON, s.writeWorkspaceMap, s.ticketContext)
	s.jira = tracker.NewJira(s.cfg)
	s.pr = forge.New(s.cfg, workspaceRoot, defaultBranch)
	s.activity = newActivityFeature(s.cfg, workspaceRoot, defaultBranch)
	s.git = &gitFeature{WorkspaceRoot: workspaceRoot}
	return s
}

func (s *Server) cfg() *config.Config { return s.cfgv.Load() }

func (s *Server) features() []plugin.Feature {
	return []plugin.Feature{
		s.jira,
		s.pr,
		s.activity,
		s.claude,
		&versionFeature{},
		s.git,
	}
}

func (s *Server) setCfg(cfg *config.Config) { s.cfgv.Store(cfg) }

func (s *Server) Handler() http.Handler {
	mux := s.buildMux()
	s.startBackground()
	return recoverPanics(mux)
}

func (s *Server) StartApp() { s.startBackground() }

func (s *Server) buildMux() *http.ServeMux {
	mux := http.NewServeMux()

	for _, register := range []func(*http.ServeMux){
		s.terminalRoutes,
		s.workspaceRoutes,
		s.serviceRoutes,
		s.sharedRoutes,
		s.configRoutes,
		s.sessionRoutes,
		s.mcpRoutes,
		s.integrationsRoutes,
		s.secretsRoutes,
		s.nmStoreRoutes,
		s.syncConfigRoutes,
		s.configBundleRoutes,
		s.doctorRoutes,
		s.logsRoutes,
		s.miscRoutes,
	} {
		register(mux)
	}
	for _, f := range s.features() {
		if hp, ok := f.(plugin.HTTPProvider); ok {
			hp.Routes(mux)
		}
	}
	return mux
}

func (s *Server) startBackground() {
	// no launch reap: a relaunch/2nd instance would SIGTERM a live instance's shells (+ claude); cleanup is on quit
	s.startResourceMonitor()
	go s.watchConfigFiles()
	go s.pr.WarmLoop()
	go s.startWebhookRelay()
	go s.startDevProxy()
	if _, ok := lock.AcquirePrimary(s.session()); !ok {
		log.Printf("pom: another runtime already owns %q — coexisting (skipping refresh-main / auto-push / port reaper)", s.session())
		return
	}
	go s.reapPortsLoop()
	go s.autoPushLoop()
	if rm, _ := s.effectiveSync(); rm {
		go s.refreshMainLoop()
	}
}

func (s *Server) watchConfigFiles() {
	path := s.configPath()
	if path == "" {
		return
	}
	dir := filepath.Dir(path)
	fingerprint := func() int64 {
		var sum int64
		for _, f := range append([]string{path}, config.FragmentPaths(dir)...) {
			if st, err := os.Stat(f); err == nil {
				sum += st.ModTime().UnixNano() ^ st.Size()
			}
		}
		return sum
	}
	last := fingerprint()
	t := time.NewTicker(2 * time.Second)
	defer t.Stop()
	for range t.C {
		cur := fingerprint()
		if cur == last {
			continue
		}
		last = cur
		if cfg, err := config.Load(path); err == nil {
			s.setCfg(cfg)
			log.Printf("pom: config changed on disk — hot-reloaded")
		}
	}
}

func recoverPanics(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer func() {
			if v := recover(); v != nil {
				log.Printf("panic in %s %s: %v", r.Method, r.URL.Path, v)
				http.Error(w, "internal error", http.StatusInternalServerError)
			}
		}()
		next.ServeHTTP(w, r)
	})
}

func (s *Server) terminalRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/panes", s.handlePanes)
	mux.HandleFunc("/api/terminals", s.handleTerminals)
	mux.HandleFunc("/api/pane/kill", s.handlePaneKill)
	mux.HandleFunc("/api/pty/busy", s.handlePaneBusy)
	mux.HandleFunc("/api/pty/reap", s.handlePtyReap)
}

func (s *Server) workspaceRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/workspaces", s.handleWorkspaces)
	mux.HandleFunc("/api/workspace/create", s.handleWorkspaceCreate)
	mux.HandleFunc("/api/workspace/delete", s.handleWorkspaceDelete)
	mux.HandleFunc("/api/workspace/rename", s.handleWorkspaceRename)
	mux.HandleFunc("/api/name/suggest", s.handleSuggestName)
	mux.HandleFunc("/api/workspace/pull-main", s.handleMainPull)
	mux.HandleFunc("/api/workspace/digest", s.handleWorkspaceDigest)
	mux.HandleFunc("/api/env/set", s.handleEnvSet)
	mux.HandleFunc("/api/environments", s.handleEnvironments)
	mux.HandleFunc("/api/repos", s.handleRepos)
	mux.HandleFunc("/api/db/reset", s.handleDBReset)
	mux.HandleFunc("/api/repo/clone", s.handleRepoClone)
}

func (s *Server) serviceRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/service/start", s.handleServiceStart)
	mux.HandleFunc("/api/service/stop", s.handleServiceStop)
	mux.HandleFunc("/api/service/restart", s.handleServiceRestart)
	mux.HandleFunc("/api/service/mode", s.handleServiceMode)
	mux.HandleFunc("/api/devproxy/log", s.handleDevProxyLog)
	mux.HandleFunc("/api/service/peek", s.handleServicePeek)
	mux.HandleFunc("/api/service/peek-all", s.handlePeekAll)
	mux.HandleFunc("/api/service/url", s.handleServiceURL)
	mux.HandleFunc("/api/shortcut/run", s.handleShortcutRun)
	mux.HandleFunc("/api/repo/open-shell", s.handleOpenShell)
	mux.HandleFunc("/api/repo/open-editor", s.handleOpenEditor)
	mux.HandleFunc("/api/editors", s.handleEditors)
	mux.HandleFunc("/api/network", s.handleNetwork)
}

func (s *Server) sharedRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/shared/logs", s.handleSharedLogs)
	mux.HandleFunc("/api/shared/inspect", s.handleSharedInspect)
	mux.HandleFunc("/api/shared/stats", s.handleSharedStats)
	mux.HandleFunc("/api/shared/status", s.handleSharedStatus)
	mux.HandleFunc("/api/shared/action", s.handleSharedAction)
	mux.HandleFunc("/api/shared/stack", s.handleSharedStack)
	mux.HandleFunc("/api/ports", s.handlePorts)
}

func (s *Server) configRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/config", s.handleConfigRead)
	mux.HandleFunc("/api/config/write", s.handleConfigWrite)
	mux.HandleFunc("/api/config/export", s.handleConfigExport)
	mux.HandleFunc("/api/config/import", s.handleConfigImport)
	mux.HandleFunc("/api/config/reload", s.handleConfigReload)
	mux.HandleFunc("/api/config/files", s.handleConfigFiles)
	mux.HandleFunc("/api/config/locate", s.handleConfigLocate)
	mux.HandleFunc("/api/config/file", s.handleConfigFile)
	mux.HandleFunc("/api/config/variables", s.handleConfigVariables)
	mux.HandleFunc("/api/config/env-override", s.handleConfigEnvOverride)
	mux.HandleFunc("/api/config/repo-env", s.handleConfigRepoEnv)
	mux.HandleFunc("/api/config/health", s.handleConfigHealth)
	mux.HandleFunc("/api/config/explain", s.handleConfigExplain)
	mux.HandleFunc("/api/config/migrate-tokens", s.handleConfigMigrateTokens)
	mux.HandleFunc("/api/config/split", s.handleConfigSplit)
	mux.HandleFunc("/api/config/normalize", s.handleConfigNormalize)
	mux.HandleFunc("/api/config/extract-preset", s.handleConfigExtractPreset)
	mux.HandleFunc("/api/lifecycle", s.handleLifecycle)
}

func (s *Server) sessionRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/session/list", s.handleSessionList)
	mux.HandleFunc("/api/session/switch", s.handleSessionSwitch)
	mux.HandleFunc("/api/session/create", s.handleSessionCreate)
	mux.HandleFunc("/api/session/delete", s.handleSessionDelete)
	mux.HandleFunc("/api/session/release-slot", s.handleSessionReleaseSlot)
}

func (s *Server) miscRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/refresh", s.handleRefresh)
	mux.HandleFunc("/api/deps", s.handleDeps)
}

type Pane struct {
	ID      string `json:"id"`
	Title   string `json:"title"`
	Session string `json:"session"`
	Window  string `json:"window"`
	Cwd     string `json:"cwd"`
	Command string `json:"command"`
}

func (s *Server) handlePanes(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"project":   s.Project,
		"version":   Version,
		"panes":     []Pane{},
		"resources": s.resources(),
	})
}

func parseAckControl(data []byte) (int, bool) {
	if !bytes.HasPrefix(data, []byte(`{"__pom":"ack"`)) {
		return 0, false
	}
	var msg struct {
		Ctrl string `json:"__pom"`
		N    int    `json:"n"`
	}
	if json.Unmarshal(data, &msg) != nil || msg.Ctrl != "ack" || msg.N <= 0 {
		return 0, false
	}
	return msg.N, true
}

func parseResizeControl(data []byte) (struct{ Cols, Rows string }, bool) {
	var out struct{ Cols, Rows string }
	if !bytes.HasPrefix(data, []byte(`{"__pom":"resize"`)) {
		return out, false
	}
	var msg struct {
		Ctrl string `json:"__pom"`
		Cols int    `json:"cols"`
		Rows int    `json:"rows"`
	}
	if json.Unmarshal(data, &msg) != nil || msg.Ctrl != "resize" || msg.Cols <= 0 || msg.Rows <= 0 {
		return out, false
	}
	out.Cols = strconv.Itoa(msg.Cols)
	out.Rows = strconv.Itoa(msg.Rows)
	return out, true
}
