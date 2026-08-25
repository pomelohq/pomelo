package core

import (
	"encoding/json"
	"fmt"
	"github.com/pomelohq/pomelo/internal/provider/shell"
	"net/http"
	"strings"
	"sync"

	"github.com/pomelohq/pomelo/internal/services"
)

type shortcutRunReq struct {
	Branch string `json:"branch"`
	IsMain bool   `json:"is_main"`
	Repo   string `json:"repo"`
	Cmd    string `json:"cmd"`
	Desc   string `json:"desc"`
}

func (s *Server) handleShortcutRun(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.cfg() == nil {
		http.Error(w, "no project config", http.StatusServiceUnavailable)
		return
	}
	var req shortcutRunReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	writeJSON(w, s.ShortcutRun(req.Branch, req.IsMain, req.Repo, req.Cmd))
}

func (s *Server) ShortcutRun(branch string, isMain bool, repo, cmd string) map[string]any {
	if s.cfg() == nil {
		return map[string]any{"ok": false, "error": "no project config"}
	}
	if cmd == "" {
		return map[string]any{"ok": false, "error": "missing cmd"}
	}
	wtPath := repoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	safe := strings.NewReplacer("/", "-", " ", "_").Replace(repo)
	services.RegenerateWorkspaceEnv(s.WorkspaceRoot, s.cfg(), branch)
	env := services.ResolveRepoEnv(s.WorkspaceRoot, s.cfg(), branch, repo)
	// cat blocks on stdin, exits only on EOF (Ctrl+D); a lone read would grab leftover input and close instantly
	shellCmd := fmt.Sprintf("%s; stty sane </dev/tty 2>/dev/null; echo; echo '— done · press Ctrl+D to close —'; cat >/dev/null 2>&1", cmd)
	holder := shellHolderName(branch, isMain, safe)
	if err := services.SpawnHolderEnv(holder, wtPath, 0, 0, shell.Command(shellCmd), env); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	return map[string]any{"ok": true, "window": holder, "pane_id": "pty:" + holder}
}

func (s *Server) handleMainPull(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.cfg() == nil {
		http.Error(w, "no project config", http.StatusServiceUnavailable)
		return
	}
	var req struct {
		Branch string `json:"branch"`
		IsMain bool   `json:"is_main"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	if !req.IsMain {
		http.Error(w, "pull-latest is only allowed on the main workspace", http.StatusBadRequest)
		return
	}
	writeJSON(w, s.MainPull(req.Branch))
}

func (s *Server) MainPull(branch string) map[string]any {
	if s.cfg() == nil {
		return map[string]any{"ok": false, "error": "no project config"}
	}
	type repoResult struct {
		Repo   string `json:"repo"`
		Branch string `json:"branch"`
		OK     bool   `json:"ok"`
		Error  string `json:"error,omitempty"`
	}

	names := s.cfg().RepoOrder
	if len(names) == 0 {
		for name := range s.cfg().Repos {
			names = append(names, name)
		}
	}

	var (
		wg      sync.WaitGroup
		mu      sync.Mutex
		results = make([]repoResult, 0, len(names))
	)
	for _, name := range names {
		dir := repoWorktreePath(s.WorkspaceRoot, name, branch, true)
		if !isGitRepo(dir) {
			continue
		}
		def := s.cfg().DefaultBranchFor(name)
		wg.Add(1)
		go func(repo, dir, def string) {
			defer wg.Done()
			res := repoResult{Repo: repo, Branch: def, OK: true}
			if err := services.ResetToDefaultAndPull(dir, def); err != nil {
				res.OK = false
				res.Error = err.Error()
			}
			mu.Lock()
			results = append(results, res)
			mu.Unlock()
		}(name, dir, def)
	}
	wg.Wait()

	allOK := true
	for _, r := range results {
		if !r.OK {
			allOK = false
			break
		}
	}
	return map[string]any{"ok": allOK, "results": results}
}
