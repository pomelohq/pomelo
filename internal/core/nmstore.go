package core

import (
	"net/http"
	"path/filepath"

	"github.com/pomelohq/pomelo/internal/services"
)

func (s *Server) nmStoreRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/nmstore/list", s.handleNMStoreList)
	mux.HandleFunc("/api/nmstore/delete", s.handleNMStoreDelete)
	mux.HandleFunc("/api/nmstore/reconcile", s.handleNMStoreReconcile)
}

func (s *Server) handleNMStoreReconcile(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.NMStoreReconcile())
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
			services.SnapshotNodeModules(r.Name, r.Path)
		}
	}
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
		out = append(out, row{NMStoreEntry: e, Current: cur, Consumers: cs})
	}
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
