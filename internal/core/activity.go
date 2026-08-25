package core

import (
	"net/http"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/pomelohq/pomelo/internal/commands"
	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/plugin"
	"github.com/pomelohq/pomelo/internal/services"
)

type activityFeature struct {
	getCfg        func() *config.Config
	WorkspaceRoot string
	DefaultBranch string

	sampler *commands.ResourceSampler
	once    sync.Once
	mu      sync.Mutex
	snap    []commands.ProcRow
}

func newActivityFeature(getCfg func() *config.Config, root, def string) *activityFeature {
	return &activityFeature{getCfg: getCfg, WorkspaceRoot: root, DefaultBranch: def, sampler: commands.NewResourceSampler()}
}

func (*activityFeature) Name() string { return "activity" }

func (f *activityFeature) Routes(mux *http.ServeMux) {
	mux.HandleFunc("/api/ps", f.handlePs)
}

func (s *Server) PsData() map[string]any { return s.activity.Ps() }

var _ plugin.HTTPProvider = (*activityFeature)(nil)

func (f *activityFeature) startLoop() {
	f.once.Do(func() {
		go func() {
			for {
				rows := f.sampler.SampleByHolder()
				f.mu.Lock()
				f.snap = rows
				f.mu.Unlock()
				time.Sleep(2 * time.Second)
			}
		}()
	})
}

type procMeta struct{ wsKey, branch, repo, svc, name string }

func (f *activityFeature) holderMeta() map[string]procMeta {
	out := map[string]procMeta{}
	cfg := f.getCfg()
	if cfg == nil || f.WorkspaceRoot == "" {
		return out
	}
	known := map[string]bool{}
	for n := range cfg.Repos {
		known[n] = true
	}
	for _, ws := range scanWorkspaces(f.WorkspaceRoot, f.DefaultBranch, known, false) {
		wsKey := "ws:" + ws.Branch
		if ws.IsMain {
			wsKey = "main:" + ws.Branch
		}
		for name, dir := range cfg.Repos {
			alias := dir.Alias
			if alias == "" {
				alias = name
			}
			for svc := range dir.Services {
				h := services.ServiceHolderName(cfg.Session, ws.Branch, name, svc)
				out[h] = procMeta{wsKey: wsKey, branch: ws.Branch, repo: alias, svc: svc, name: alias + "/" + svc}
			}
		}
		for _, svc := range cfg.WsServiceOrder {
			h := services.WsServiceHolderName(cfg.Session, ws.Branch, svc)
			out[h] = procMeta{wsKey: wsKey, branch: ws.Branch, svc: svc, name: svc}
		}
	}
	return out
}

type wsInfo struct{ branch, wsKey, safe string }

func (f *activityFeature) workspaceList() []wsInfo {
	cfg := f.getCfg()
	if cfg == nil || f.WorkspaceRoot == "" {
		return nil
	}
	known := map[string]bool{}
	for n := range cfg.Repos {
		known[n] = true
	}
	var out []wsInfo
	for _, ws := range scanWorkspaces(f.WorkspaceRoot, f.DefaultBranch, known, false) {
		key := "ws:" + ws.Branch
		if ws.IsMain {
			key = "main:" + ws.Branch
		}
		out = append(out, wsInfo{branch: ws.Branch, wsKey: key, safe: services.BranchSafe(ws.Branch)})
	}
	sort.Slice(out, func(i, j int) bool { return len(out[i].branch) > len(out[j].branch) })
	return out
}

func friendlyHolder(label string) (name, kind string) {
	if strings.Contains(label, "claude") {
		return "Claude", "claude"
	}
	return services.HolderFor(label).Display()
}

func (f *activityFeature) handlePs(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, f.Ps())
}

func (f *activityFeature) Ps() map[string]any {
	f.startLoop()
	f.mu.Lock()
	rows := f.snap
	f.mu.Unlock()
	meta := f.holderMeta()

	type P struct {
		PID    int     `json:"pid"`
		Label  string  `json:"label"`
		Name   string  `json:"name"`
		WsKey  string  `json:"ws_key"`
		Branch string  `json:"branch"`
		Repo   string  `json:"repo"`
		Svc    string  `json:"svc"`
		Kind   string  `json:"kind"`
		CPU    float64 `json:"cpu"`
		RAMMB  float64 `json:"ram_mb"`
	}
	wss := f.workspaceList()
	out := []P{}
	var totCPU, totRAM float64
	for _, row := range rows {
		p := P{PID: row.PID, Label: row.Label, Kind: row.Kind, CPU: row.CPU, RAMMB: row.RAMMB, Name: row.Label}
		if m, ok := meta[row.Label]; ok {
			p.Name, p.WsKey, p.Branch, p.Repo, p.Svc = m.name, m.wsKey, m.branch, m.repo, m.svc
		} else {
			for _, ws := range wss {
				if strings.Contains(row.Label, ws.branch) || (ws.safe != "" && strings.Contains(row.Label, ws.safe)) {
					p.WsKey, p.Branch = ws.wsKey, ws.branch
					break
				}
			}
			p.Name, p.Kind = friendlyHolder(row.Label)
		}
		out = append(out, p)
		totCPU += row.CPU
		totRAM += row.RAMMB
	}
	return map[string]any{
		"processes": out,
		"total":     map[string]any{"cpu": totCPU, "ram_mb": totRAM, "procs": len(out)},
	}
}
