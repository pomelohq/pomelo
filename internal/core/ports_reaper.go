package core

import (
	"log"
	"time"

	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

// pomAllocatedPorts is the set of ports Pomelo currently owns in its registry —
// the reaper only ever reclaims listeners on these, never a user's own process.
func pomAllocatedPorts() map[int]bool {
	set := map[int]bool{}
	for _, l := range services.PortMgr().Snapshot() {
		if l.Port > 0 {
			set[l.Port] = true
		}
	}
	return set
}

func (s *Server) reapPortsLoop() {
	t := time.NewTicker(5 * time.Second)
	defer t.Stop()
	n := 0
	for range t.C {
		services.PortMgr().Reap()
		if n++; n%6 == 0 && s.WorkspaceRoot != "" {
			if reaped := ptyhost.ReapOrphanServices(s.WorkspaceRoot, pomAllocatedPorts()); len(reaped) > 0 {
				log.Printf("reaped %d orphaned service process(es): %v", len(reaped), reaped)
			}
		}
	}
}
