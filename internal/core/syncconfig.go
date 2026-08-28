package core

import (
	"net/http"
	"time"

	"github.com/pomelohq/pomelo/internal/appstate"
)

func (s *Server) syncConfigRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/sync/config", s.handleSyncConfig)
}

func (s *Server) handleSyncConfig(w http.ResponseWriter, r *http.Request) {
	if r.Method == http.MethodPost {
		req, err := readJSON[struct {
			RefreshMain        bool `json:"refresh_main"`
			RefreshIntervalSec int  `json:"refresh_interval_sec"`
		}](r)
		if err != nil {
			httpErr(w, http.StatusBadRequest, "bad json")
			return
		}
		if err := s.SyncSet(req.RefreshMain, req.RefreshIntervalSec); err != nil {
			httpErr(w, http.StatusInternalServerError, "%s", err.Error())
			return
		}
		writeJSON(w, map[string]any{"ok": true})
		return
	}
	writeJSON(w, s.SyncGet())
}

func (s *Server) SyncGet() map[string]any {
	rm, iv := s.effectiveSync()
	if iv == 0 {
		iv = 1800
	}
	// Compute the next boundary deterministically each call rather than reading the
	// scheduler's stored time, so the countdown never sticks (works even for a
	// non-primary instance whose scheduler doesn't run).
	next := int64(0)
	if rm && !s.syncPulling.Load() {
		next = nextAlignedRun(time.Now(), iv).Unix()
	}
	prog := []RepoPull{}
	if p := s.syncProg.Load(); p != nil {
		prog = *p
	}
	return map[string]any{"refresh_main": rm, "refresh_interval_sec": iv, "next_run_at": next, "pulling": s.syncPulling.Load(), "last_pull_at": s.syncLastPull.Load(), "progress": prog}
}

func (s *Server) SyncSet(refreshMain bool, intervalSec int) error {
	st := appstate.Load(s.session())
	st.Sync = appstate.SyncConfig{Configured: true, RefreshMain: refreshMain, RefreshIntervalSec: intervalSec}
	if err := appstate.Save(s.session(), st); err != nil {
		return err
	}
	select {
	case s.syncReset <- struct{}{}: // reschedule at once
	default:
	}
	return nil
}
