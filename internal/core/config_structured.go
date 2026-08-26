package core

import "sort"

type cfgSvcOverview struct {
	Name string `json:"name"`
	Cmd  string `json:"cmd"`
	Dir  string `json:"dir"`
	Port bool   `json:"port"`
}

type cfgRepoOverview struct {
	Name      string           `json:"name"`
	Alias     string           `json:"alias"`
	Services  []cfgSvcOverview `json:"services"`
	Setup     []string         `json:"setup"`
	Databases []string         `json:"databases"`
	Shared    []string         `json:"shared"`
}

type cfgSharedOverview struct {
	Name string `json:"name"`
	Type string `json:"type"`
}

type cfgOverview struct {
	Session string              `json:"session"`
	Repos   []cfgRepoOverview   `json:"repos"`
	Shared  []cfgSharedOverview `json:"shared"`
}

// ConfigStructured renders the loaded config as a UI-friendly overview: repos ->
// services/setup/databases/shared-refs, plus the shared services. Read-only; the
// YAML editor stays the source of truth for edits (round-tripping a form back to
// YAML would lose comments and key order).
func (s *Server) ConfigStructured() cfgOverview {
	cfg := s.cfg()
	ov := cfgOverview{Repos: []cfgRepoOverview{}, Shared: []cfgSharedOverview{}}
	if cfg == nil {
		return ov
	}
	ov.Session = cfg.Session
	for _, name := range sortedKeys(cfg.Repos) {
		d := cfg.Repos[name]
		ro := cfgRepoOverview{Name: name, Alias: d.Alias, Setup: d.Setup,
			Services: []cfgSvcOverview{}, Databases: []string{}, Shared: []string{}}
		for _, svcName := range sortedKeys(d.Services) {
			sv := d.Services[svcName]
			ro.Services = append(ro.Services, cfgSvcOverview{
				Name: svcName, Cmd: sv.ActiveCmd(""), Dir: sv.Dir, Port: sv.HasPort()})
		}
		for db := range d.Databases {
			ro.Databases = append(ro.Databases, db)
		}
		sort.Strings(ro.Databases)
		for _, r := range d.SharedSvcRefs {
			ro.Shared = append(ro.Shared, r.Name)
		}
		ov.Repos = append(ov.Repos, ro)
	}
	for _, name := range sortedKeys(cfg.SharedServices) {
		ov.Shared = append(ov.Shared, cfgSharedOverview{Name: name, Type: cfg.SharedServices[name].Type})
	}
	return ov
}
