package core

import (
	"net/http"

	"github.com/pomelohq/pomelo/internal/services"
)

func (s *Server) nmStoreRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/nmstore/list", s.handleNMStoreList)
	mux.HandleFunc("/api/nmstore/delete", s.handleNMStoreDelete)
}

func (s *Server) handleNMStoreList(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.NMStoreList())
}

func (s *Server) NMStoreList() map[string]any {
	entries := services.NMStoreEntries()

	// Which workspace/repo currently resolves to each cached (repo, hash): a
	// workspace "uses" a cache if that repo's lockfile in its worktree hashes to it.
	type consumer struct {
		Branch string `json:"branch"`
		IsMain bool   `json:"is_main"`
	}
	consumers := map[string][]consumer{}
	for _, ws := range s.collectWorkspaces(false, true) {
		for _, r := range ws.Repos {
			h := services.LockHash(r.Path)
			if h == "" {
				continue
			}
			k := r.Name + "/" + h
			consumers[k] = append(consumers[k], consumer{Branch: ws.Branch, IsMain: ws.IsMain})
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
	return map[string]any{"entries": out, "total": total}
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
