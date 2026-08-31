package core

import (
	"encoding/json"
	"log"
	"net/http"
	"strings"

	"github.com/pomelohq/pomelo/internal/commands"
	"github.com/pomelohq/pomelo/internal/pipeline"
	"github.com/pomelohq/pomelo/internal/services"
)

// CreateWorkspaceRemote kicks off a workspace create for the remote (phone). It
// runs the pipeline in the background and returns immediately; the phone polls
// the workspace list to see it appear (create can take minutes).
func (s *Server) CreateWorkspaceRemote(branch string, repos []string, displayName string) map[string]any {
	if s.cfg() == nil {
		return map[string]any{"ok": false, "error": "no project config"}
	}
	branch = strings.TrimSpace(branch)
	if branch == "" {
		return map[string]any{"ok": false, "error": "branch required"}
	}
	combo := ""
	for k := range s.cfg().AllWorkspaces() {
		combo = k
		break
	}
	cfgPath := s.WorkspaceRoot + "/pom.yml"
	cfg := s.cfg()
	reposCSV := strings.Join(repos, ",")
	dn := strings.TrimSpace(displayName)
	go func() {
		if err := commands.WorkspaceCreate(cfg, cfgPath, combo, branch, 0, reposCSV, "", false); err != nil {
			log.Printf("remote create workspace %q: %v", branch, err)
			return
		}
		if dn != "" {
			s.WorkspaceRename(branch, false, dn)
		}
	}()
	return map[string]any{"ok": true, "branch": branch}
}

type createReq struct {
	Combo  string   `json:"combo"`
	Branch string   `json:"branch"`
	Repos  []string `json:"repos,omitempty"`
	Env    string   `json:"env,omitempty"`
	NoSeed bool     `json:"no_seed,omitempty"`
}

func (s *Server) handleWorkspaceCreate(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.cfg() == nil {
		http.Error(w, "no project config", http.StatusServiceUnavailable)
		return
	}
	var req createReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	if req.Branch == "" {
		http.Error(w, "branch required", http.StatusBadRequest)
		return
	}
	combo := req.Combo
	if combo == "" {
		for k := range s.cfg().AllWorkspaces() {
			combo = k
			break
		}
	}
	cfgPath := s.WorkspaceRoot + "/pom.yml"
	repos := strings.Join(req.Repos, ",")
	if err := commands.WorkspaceCreate(s.cfg(), cfgPath, combo, req.Branch, 0, repos, req.Env, req.NoSeed); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "branch": req.Branch})
}

func (s *Server) StreamCreate(send func(map[string]any), gitBranch, combo, repos, env, baseWS, baseRef string) {
	branch := s.workspaceNameFor(gitBranch)
	if combo == "" {
		for k := range s.cfg().AllWorkspaces() {
			combo = k
			break
		}
	}
	cfgPath := s.WorkspaceRoot + "/pom.yml"
	var selected []services.DirBranch
	if repos != "" {
		for _, entry := range strings.Split(repos, ",") {
			parts := strings.SplitN(entry, ":", 2)
			if len(parts) == 2 {
				selected = append(selected, services.DirBranch{Name: parts[0], Branch: parts[1]})
			} else {
				selected = append(selected, services.DirBranch{Name: parts[0], Branch: gitBranch})
			}
		}
	}

	var ctx *pipeline.CreateContext
	var err error
	if len(selected) > 0 {
		ctx, err = pipeline.FromConfigWithSelection(s.cfg(), cfgPath, combo, branch, selected)
	} else {
		ctx, err = pipeline.FromConfig(s.cfg(), cfgPath, combo, branch, nil)
	}
	if err != nil {
		send(map[string]any{"type": "pipeline-failed", "error": err.Error()})
		return
	}
	ctx.Environment = env
	if branch != gitBranch {
		ctx.GitBranch = gitBranch
	}

	ch := make(chan pipeline.Event, 16)
	go pipeline.RunCreatePipeline(ctx, ch)

	send(map[string]any{
		"type":   "pipeline-started",
		"branch": branch, "combo": combo,
		"total":  len(pipeline.AllCreateStages),
		"stages": createStageLabels(),
	})

	for evt := range ch {
		if p := pipelineEventJSON(evt); p != nil {
			send(p)
		}
	}
}

// StreamAddRepo adds repos to an existing workspace by running the create
// pipeline scoped to just the new repos: their worktrees fork onto the workspace
// branch and get env/ports/services, while the existing repos are untouched.
func (s *Server) StreamAddRepo(send func(map[string]any), branch string, isMain bool, repos string) {
	if s.cfg() == nil {
		send(map[string]any{"type": "pipeline-failed", "error": "no project config"})
		return
	}
	combo := ""
	for k := range s.cfg().AllWorkspaces() {
		combo = k
		break
	}

	var selected []services.DirBranch
	for _, entry := range strings.Split(repos, ",") {
		name := strings.TrimSpace(entry)
		if name == "" {
			continue
		}
		if _, ok := s.cfg().Repos[name]; !ok {
			continue
		}
		if services.DirExists(services.RepoWorktreePath(s.WorkspaceRoot, name, branch, isMain)) {
			continue // already in this workspace
		}
		selected = append(selected, services.DirBranch{Name: name, Branch: branch})
	}
	if len(selected) == 0 {
		send(map[string]any{"type": "pipeline-failed", "error": "no new repos to add"})
		return
	}

	cfgPath := s.WorkspaceRoot + "/pom.yml"
	ctx, err := pipeline.FromConfigWithSelection(s.cfg(), cfgPath, combo, branch, selected)
	if err != nil {
		send(map[string]any{"type": "pipeline-failed", "error": err.Error()})
		return
	}

	ch := make(chan pipeline.Event, 16)
	go pipeline.RunCreatePipeline(ctx, ch)
	send(map[string]any{
		"type": "pipeline-started", "branch": branch, "combo": combo,
		"total": len(pipeline.AllCreateStages), "stages": createStageLabels(),
	})
	for evt := range ch {
		if p := pipelineEventJSON(evt); p != nil {
			send(p)
		}
	}
}

func createStageLabels() []string {
	out := make([]string, len(pipeline.AllCreateStages))
	for i, st := range pipeline.AllCreateStages {
		out[i] = st.Label()
	}
	return out
}

func deleteStageLabels() []string {
	out := make([]string, len(pipeline.AllDeleteStages))
	for i, st := range pipeline.AllDeleteStages {
		out[i] = st.Label()
	}
	return out
}

func pipelineEventJSON(evt pipeline.Event) map[string]any {
	switch evt.Type {
	case pipeline.EventStageStarted:
		return map[string]any{"type": "stage-started", "index": evt.Index, "total": evt.Total, "name": evt.Name}
	case pipeline.EventStageCompleted:
		return map[string]any{"type": "stage-completed"}
	case pipeline.EventStageSkipped:
		return map[string]any{"type": "stage-skipped", "index": evt.Index}
	case pipeline.EventStageProgress:
		return map[string]any{"type": "stage-progress", "detail": evt.Detail}
	case pipeline.EventPipelineCompleted:
		return map[string]any{"type": "pipeline-completed"}
	case pipeline.EventPipelineFailed:
		return map[string]any{"type": "pipeline-failed", "index": evt.Index, "error": evt.Error}
	}
	return nil
}

type deleteReq struct {
	Branch string `json:"branch"`
}

func (s *Server) handleWorkspaceDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.cfg() == nil {
		http.Error(w, "no project config", http.StatusServiceUnavailable)
		return
	}
	var req deleteReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	if req.Branch == "" {
		http.Error(w, "branch required", http.StatusBadRequest)
		return
	}
	cfgPath := s.WorkspaceRoot + "/pom.yml"
	if err := commands.WorkspaceDelete(s.cfg(), cfgPath, req.Branch); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "branch": req.Branch})
}

type renameReq struct {
	Branch      string `json:"branch"`
	IsMain      bool   `json:"is_main"`
	DisplayName string `json:"display_name"`
}

func (s *Server) handleWorkspaceRename(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.WorkspaceRoot == "" {
		http.Error(w, "no project", http.StatusServiceUnavailable)
		return
	}
	var req renameReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	writeJSON(w, s.WorkspaceRename(req.Branch, req.IsMain, req.DisplayName))
}

func (s *Server) WorkspaceRename(branch string, isMain bool, displayName string) map[string]any {
	if s.WorkspaceRoot == "" {
		return map[string]any{"ok": false, "error": "no project"}
	}
	if branch == "" {
		return map[string]any{"ok": false, "error": "branch required"}
	}
	name := strings.TrimSpace(displayName)
	if len([]rune(name)) > 60 {
		name = string([]rune(name)[:60])
	}
	folder := services.WorkspaceRootDir(s.WorkspaceRoot, branch, isMain)
	if !services.DirExists(folder) {
		return map[string]any{"ok": false, "error": "workspace not found"}
	}
	if err := services.SetWorkspaceDisplayName(folder, name); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	return map[string]any{"ok": true, "branch": branch, "display_name": name}
}

func (s *Server) StreamDelete(send func(map[string]any), branch string) {
	cfgPath := s.WorkspaceRoot + "/pom.yml"
	ctx := commands.BuildDeleteContext(s.cfg(), cfgPath, branch)

	ch := make(chan pipeline.Event, 16)
	go pipeline.RunDeletePipeline(ctx, ch)

	send(map[string]any{"type": "pipeline-started", "branch": branch, "total": len(pipeline.AllDeleteStages), "stages": deleteStageLabels()})
	for evt := range ch {
		if p := pipelineEventJSON(evt); p != nil {
			send(p)
		}
	}
}
