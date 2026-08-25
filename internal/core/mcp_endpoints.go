package core

import (
	"context"
	"github.com/pomelohq/pomelo/internal/provider/shell"
	"net/http"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"time"

	"github.com/pomelohq/pomelo/internal/services"
)

func (s *Server) mcpRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/databases", s.handleDatabases)
	mux.HandleFunc("/api/run-in-env", s.handleRunInEnv)
	mux.HandleFunc("/api/ports/relocate", s.handlePortsRelocate)
	mux.HandleFunc("/api/db/list", s.handleDBListHTTP)
	mux.HandleFunc("/api/db/tables", s.handleDBTablesHTTP)
	mux.HandleFunc("/api/db/columns", s.handleDBColumnsHTTP)
	mux.HandleFunc("/api/db/query", s.handleDBQueryHTTP)
}

func (s *Server) handleDBListHTTP(w http.ResponseWriter, r *http.Request) {
	if s.cfg() == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project config")
		return
	}
	branch := r.URL.Query().Get("branch")
	if branch == "" {
		httpErr(w, http.StatusBadRequest, "branch required")
		return
	}
	writeJSON(w, s.DBList(branch))
}

func (s *Server) handleDBTablesHTTP(w http.ResponseWriter, r *http.Request) {
	if s.cfg() == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project config")
		return
	}
	q := r.URL.Query()
	branch, db := q.Get("branch"), q.Get("db")
	if branch == "" || db == "" {
		httpErr(w, http.StatusBadRequest, "branch + db required")
		return
	}
	writeJSON(w, s.DBTables(branch, db))
}

func (s *Server) handleDBColumnsHTTP(w http.ResponseWriter, r *http.Request) {
	if s.cfg() == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project config")
		return
	}
	q := r.URL.Query()
	branch, db := q.Get("branch"), q.Get("db")
	if branch == "" || db == "" {
		httpErr(w, http.StatusBadRequest, "branch + db required")
		return
	}
	writeJSON(w, s.DBColumns(branch, db))
}

type dbQueryReq struct {
	Branch string `json:"branch"`
	DB     string `json:"db"`
	SQL    string `json:"sql"`
	Limit  int    `json:"limit"`
}

func (s *Server) handleDBQueryHTTP(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	if s.cfg() == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project config")
		return
	}
	req, err := readJSON[dbQueryReq](r)
	if err != nil || req.Branch == "" || req.DB == "" || strings.TrimSpace(req.SQL) == "" {
		httpErr(w, http.StatusBadRequest, "branch + db + sql required")
		return
	}
	writeJSON(w, s.DBQuery(req.Branch, req.DB, req.SQL, req.Limit))
}

type dbEntry struct {
	Name string `json:"name"`
	URL  string `json:"url"`
}

func (s *Server) handleDatabases(w http.ResponseWriter, r *http.Request) {
	if s.cfg() == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project config")
		return
	}
	branch := r.URL.Query().Get("branch")
	if branch == "" {
		httpErr(w, http.StatusBadRequest, "branch required")
		return
	}
	host, port, user, pw := s.pgConn()
	out := []dbEntry{}
	for _, name := range s.databasesFor(branch) {
		out = append(out, dbEntry{
			Name: name,
			URL:  urlForDB(user, pw, host, int(port), name),
		})
	}
	writeJSON(w, map[string]any{"databases": out})
}

func urlForDB(user, pw, host string, port int, db string) string {
	var b strings.Builder
	b.WriteString("postgres://")
	b.WriteString(user)
	if pw != "" {
		b.WriteString(":" + pw)
	}
	b.WriteString("@" + host)
	if port != 0 {
		b.WriteString(":" + strconv.Itoa(port))
	}
	b.WriteString("/" + db)
	return b.String()
}

type runInEnvReq struct {
	Branch string `json:"branch"`
	IsMain bool   `json:"is_main"`
	Repo   string `json:"repo"`
	Cmd    string `json:"cmd"`
}

func (s *Server) handleRunInEnv(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	if s.cfg() == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project config")
		return
	}
	req, err := readJSON[runInEnvReq](r)
	if err != nil || strings.TrimSpace(req.Cmd) == "" {
		httpErr(w, http.StatusBadRequest, "branch + cmd required")
		return
	}
	if req.IsMain {
		httpErr(w, http.StatusForbidden, "main is read-only — run this in a test branch")
		return
	}
	wt := repoWorktreePath(s.WorkspaceRoot, req.Repo, req.Branch, req.IsMain)
	if !dirExists(wt) {
		httpErr(w, http.StatusBadRequest, "no worktree for repo %q on %q", req.Repo, req.Branch)
		return
	}
	services.RegenerateWorkspaceEnv(s.WorkspaceRoot, s.cfg(), req.Branch)
	env := services.ResolveRepoEnv(s.WorkspaceRoot, s.cfg(), req.Branch, req.Repo)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()
	login := shell.Login(req.Cmd)
	cmd := exec.CommandContext(ctx, login[0], login[1:]...)
	cmd.Dir = wt
	cmd.Env = append(os.Environ(), env...)
	out, runErr := cmd.CombinedOutput()
	exit := 0
	if ee, ok := runErr.(*exec.ExitError); ok {
		exit = ee.ExitCode()
	} else if runErr != nil {
		exit = -1
	}
	writeJSON(w, map[string]any{"exit": exit, "output": string(out)})
}

type relocateReq struct {
	Branch string `json:"branch"`
}

func (s *Server) handlePortsRelocate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	if s.cfg() == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project config")
		return
	}
	req, err := readJSON[relocateReq](r)
	if err != nil || req.Branch == "" {
		httpErr(w, http.StatusBadRequest, "branch required")
		return
	}
	wsKey := services.PortWsKey(req.Branch)
	block := services.RelocateToCleanBlock(s.WorkspaceRoot, wsKey)
	if block < 0 {
		writeJSON(w, map[string]any{"ok": false, "error": "no free port region left — free some ports or run pom refresh"})
		return
	}
	services.RegenerateWorkspaceEnv(s.WorkspaceRoot, s.cfg(), req.Branch)
	writeJSON(w, map[string]any{"ok": true, "block": block})
}
