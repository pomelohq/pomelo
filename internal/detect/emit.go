package detect

import (
	"path/filepath"
	"sort"
	"strconv"

	"gopkg.in/yaml.v3"
)

// RepoDetection is everything detected for one cloned repo: its runnable apps
// (from DetectRepo) and its backing services (shared ComposeServices from
// ParseCompose).
type RepoDetection struct {
	Name   string
	Alias  string // optional; defaults to Name
	Apps   []StackFacts
	Shared []ComposeService
}

// Emit renders a draft pom.yml from detection results. It emits the structure
// (repos → services with run commands, setup, shared_services) — a starting point
// the onboarding agent refines (env wiring is left to Phase 1). The output is
// valid config: it loads and validates cleanly.
func Emit(session string, repos []RepoDetection) (string, error) {
	cfg := emitConfig{Session: session, DefaultBranch: "main"}
	sharedSeen := map[string]*emitShared{}

	for _, rd := range repos {
		al := rd.Alias
		if al == "" {
			al = rd.Name
		}
		er := &emitRepo{Alias: al}
		used := map[string]bool{}
		var setup []string
		setupSeen := map[string]bool{}

		for _, app := range rd.Apps {
			if app.Install != "" && !setupSeen[app.Install] {
				setupSeen[app.Install] = true
				setup = append(setup, app.Install)
			}
			for _, s := range app.Setup {
				if !setupSeen[s] {
					setupSeen[s] = true
					setup = append(setup, s)
				}
			}
			for _, run := range app.Run {
				name := serviceName(app, run, used)
				used[name] = true
				svc := &emitService{Cmd: run.Cmd}
				if app.Dir != "" {
					svc.Dir = app.Dir
				}
				if run.Kind == "server" && app.Port > 0 {
					yes := true
					svc.Port = &yes
				}
				if er.Services == nil {
					er.Services = map[string]*emitService{}
				}
				er.Services[name] = svc
			}
		}
		er.Setup = setup

		for _, sh := range rd.Shared {
			if sh.Kind != KindShared || sh.Type == "custom" || sh.Type == "" {
				continue
			}
			er.SharedServices = appendUniq(er.SharedServices, sh.Type)
			if _, ok := sharedSeen[sh.Type]; !ok {
				sharedSeen[sh.Type] = &emitShared{Type: sh.Type}
			}
		}
		sort.Strings(er.SharedServices)

		if cfg.Repos == nil {
			cfg.Repos = map[string]*emitRepo{}
		}
		cfg.Repos[rd.Name] = er
	}

	if len(sharedSeen) > 0 {
		cfg.SharedServices = sharedSeen
	}

	out, err := yaml.Marshal(cfg)
	if err != nil {
		return "", err
	}
	return string(out), nil
}

type emitConfig struct {
	Session        string                 `yaml:"session,omitempty"`
	DefaultBranch  string                 `yaml:"default_branch,omitempty"`
	Repos          map[string]*emitRepo   `yaml:"repos,omitempty"`
	SharedServices map[string]*emitShared `yaml:"shared_services,omitempty"`
}

type emitRepo struct {
	Alias          string                  `yaml:"alias,omitempty"`
	Services       map[string]*emitService `yaml:"services,omitempty"`
	Setup          []string                `yaml:"setup,omitempty"`
	SharedServices []string                `yaml:"shared_services,omitempty"`
}

type emitService struct {
	Cmd  string `yaml:"cmd"`
	Dir  string `yaml:"dir,omitempty"`
	Port *bool  `yaml:"port,omitempty"`
}

type emitShared struct {
	Type string `yaml:"type,omitempty"`
}

// serviceName derives a stable, unique service name from an app + run.
func serviceName(app StackFacts, run ResolvedRun, used map[string]bool) string {
	base := app.Framework
	if app.Dir != "" {
		base = filepath.Base(app.Dir)
	}
	if base == "" {
		base = app.Language
	}
	if base == "" {
		base = "app"
	}
	if run.Kind == "worker" {
		base += "-worker"
	}
	name := base
	for i := 2; used[name]; i++ {
		name = base + "-" + strconv.Itoa(i)
	}
	return name
}

func appendUniq(s []string, v string) []string {
	for _, x := range s {
		if x == v {
			return s
		}
	}
	return append(s, v)
}
