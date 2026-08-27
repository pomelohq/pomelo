package core

import (
	"net/http"
	"path/filepath"
	"sort"
	"sync"

	"github.com/pomelohq/pomelo/internal/services"
)

var (
	nmProgMu sync.Mutex
	nmProg   string
)

func setNMProg(s string) { nmProgMu.Lock(); nmProg = s; nmProgMu.Unlock() }

func (s *Server) NMStoreProgress() string { nmProgMu.Lock(); defer nmProgMu.Unlock(); return nmProg }

func (s *Server) nmStoreRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/nmstore/list", s.handleNMStoreList)
	mux.HandleFunc("/api/nmstore/delete", s.handleNMStoreDelete)
	mux.HandleFunc("/api/nmstore/reconcile", s.handleNMStoreReconcile)
	mux.HandleFunc("/api/nmstore/reclaim", s.handleNMStoreReclaim)
}

func (s *Server) handleNMStoreReconcile(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.NMStoreReconcile())
}

func (s *Server) handleNMStoreReclaim(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.NMStoreReclaim())
}

// NMStoreReclaim relinks each workspace's node_modules to the shared store copy (CoW);
// rewrites worktree node_modules. Reports disk freed via free-space delta.
func (s *Server) NMStoreReclaim() map[string]any {
	s.NMStoreReconcile()
	before := services.FreeBytes(s.WorkspaceRoot)
	var targets []services.NMReclaimTarget
	for _, ws := range s.collectWorkspaces(false, true) {
		for _, r := range ws.Repos {
			targets = append(targets, services.NMReclaimTarget{Repo: r.Name, Branch: ws.Branch, Worktree: r.Path})
		}
	}
	relinked := services.ReclaimNodeModules(targets, func(repo, branch string) {
		setNMProg("Deduping " + repo + " (" + branch + ")")
	})
	setNMProg("")
	reclaimed := services.FreeBytes(s.WorkspaceRoot) - before
	if reclaimed < 0 {
		reclaimed = 0
	}
	return map[string]any{"ok": true, "relinked": relinked, "reclaimed": reclaimed}
}

// NMStoreReconcile snapshots hand-installed node_modules (e.g. `npm install` run in a
// terminal) into the store. Copies into the store only; never touches a worktree.
func (s *Server) NMStoreReconcile() map[string]any {
	before := map[string]bool{}
	for _, e := range services.NMStoreEntries() {
		before[e.Repo+"/"+e.Hash] = true
	}
	for _, ws := range s.collectWorkspaces(false, true) {
		for _, r := range ws.Repos {
			setNMProg("Caching " + r.Name + " (" + ws.Branch + ")")
			services.SnapshotNodeModules(r.Name, r.Path)
		}
	}
	setNMProg("")
	added, addedBytes := 0, int64(0)
	for _, e := range services.NMStoreEntries() {
		if !before[e.Repo+"/"+e.Hash] {
			added++
			addedBytes += e.Bytes
		}
	}
	return map[string]any{"ok": true, "added": added, "bytes": addedBytes}
}

func (s *Server) handleNMStoreList(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.NMStoreList())
}

func (s *Server) NMStoreList() map[string]any {
	entries := services.NMStoreEntries()
	// The nm-store is global (keyed by repo name), so it holds caches from every
	// project. Scope the board to the current project's repos, else another
	// project's repos (e.g. web/jobs) show up here as "unused" and could be reclaimed.
	if cfg := s.cfg(); cfg != nil && len(cfg.Repos) > 0 {
		mine := map[string]bool{}
		for name := range cfg.Repos {
			mine[name] = true
		}
		kept := entries[:0]
		for _, e := range entries {
			if mine[e.Repo] {
				kept = append(kept, e)
			}
		}
		entries = kept
	}
	inStore := map[string]bool{}
	for _, e := range entries {
		inStore[e.Repo+"/"+e.Hash] = true
	}

	type consumer struct {
		Branch string `json:"branch"`
		IsMain bool   `json:"is_main"`
	}
	// A workspace "uses" a cache if that repo's lockfile hashes to it. Unoptimized =
	// the repo has node_modules on disk whose hash was never captured into the store.
	type unopt struct {
		Branch string `json:"branch"`
		IsMain bool   `json:"is_main"`
		Repo   string `json:"repo"`
		Hash   string `json:"hash"`
	}
	consumers := map[string][]consumer{}
	var unoptimized []unopt
	for _, ws := range s.collectWorkspaces(false, true) {
		for _, r := range ws.Repos {
			h := services.LockHash(r.Path)
			if h == "" {
				continue
			}
			k := r.Name + "/" + h
			if inStore[k] {
				consumers[k] = append(consumers[k], consumer{Branch: ws.Branch, IsMain: ws.IsMain})
			} else if services.DirExists(filepath.Join(r.Path, "node_modules")) {
				unoptimized = append(unoptimized, unopt{Branch: ws.Branch, IsMain: ws.IsMain, Repo: r.Name, Hash: h})
			}
		}
	}

	type row struct {
		services.NMStoreEntry
		Current   bool       `json:"current"`
		Orphan    bool       `json:"orphan"`
		Consumers []consumer `json:"consumers"`
	}
	out := make([]row, 0, len(entries))
	var total int64
	for _, e := range entries {
		total += e.Bytes
		cs := consumers[e.Repo+"/"+e.Hash]
		if cs == nil {
			cs = []consumer{}
		}
		cur := false
		for _, c := range cs {
			if c.IsMain {
				cur = true
			}
		}
		out = append(out, row{NMStoreEntry: e, Current: cur, Orphan: len(cs) == 0, Consumers: cs})
	}
	// Core owns ordering (ADR 0001): in-use first, then largest — the UI just renders.
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Current != out[j].Current {
			return out[i].Current
		}
		return out[i].Bytes > out[j].Bytes
	})
	if unoptimized == nil {
		unoptimized = []unopt{}
	}
	return map[string]any{"entries": out, "total": total, "unoptimized": unoptimized}
}

func (s *Server) handleNMStoreDelete(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	req, err := readJSON[struct {
		Repo string `json:"repo"`
		Hash string `json:"hash"`
	}](r)
	if err != nil {
		httpErr(w, http.StatusBadRequest, "bad json")
		return
	}
	if err := s.NMStoreDelete(req.Repo, req.Hash); err != nil {
		httpErr(w, http.StatusBadRequest, "%s", err.Error())
		return
	}
	writeJSON(w, map[string]any{"ok": true})
}

func (s *Server) NMStoreDelete(repo, hash string) error {
	return services.RemoveNMStoreEntry(repo, hash)
}
