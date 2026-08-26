package core

import (
	"encoding/json"
	"net/http"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strings"
	"sync"

	"github.com/pomelohq/pomelo/internal/agent/codeagent"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
	"github.com/pomelohq/pomelo/internal/workspace"
)

type Workspace struct {
	Branch      string    `json:"branch"`
	DisplayName string    `json:"display_name,omitempty"`
	IsMain      bool      `json:"is_main"`
	Path        string    `json:"path"`
	Repos       []Repo    `json:"repos"`
	WsServices  []Service `json:"ws_services,omitempty"`
	Agents      []Agent   `json:"agents"`
	Running     int       `json:"running"`
	Total       int       `json:"total"`
}

type Repo struct {
	Name        string     `json:"name"`
	Alias       string     `json:"alias"`
	Path        string     `json:"path"`
	Branch      string     `json:"branch"`
	Dirty       int        `json:"dirty"`
	LastSubject string     `json:"last_subject"`
	PaneID      string     `json:"pane_id,omitempty"`
	Services    []Service  `json:"services,omitempty"`
	Shortcuts   []Shortcut `json:"shortcuts,omitempty"`
	Setup       []string   `json:"setup,omitempty"`
}

type Service struct {
	Name       string   `json:"name"`
	Running    bool     `json:"running"`
	TmuxWindow string   `json:"tmux_window,omitempty"`
	Mode       string   `json:"mode,omitempty"`
	Modes      []string `json:"modes,omitempty"`
	AgentName  string   `json:"agent_name,omitempty"`
	AgentState string   `json:"agent_state,omitempty"`
	Env        string   `json:"env,omitempty"`
	Profiles   []string `json:"profiles,omitempty"`
	Port       int      `json:"port,omitempty"`
	Crashed    bool     `json:"crashed,omitempty"`
	CrashLog   string   `json:"crash_log,omitempty"`
}

var reCrashANSI = regexp.MustCompile(`\x1b\[[0-9;?]*[ -/]*[@-~]`)

func serviceCrash(holder string) (bool, string) {
	crashed, out := ptyhost.CrashInfo(holder)
	if !crashed || len(out) == 0 {
		return false, ""
	}
	return true, lastLines(reCrashANSI.ReplaceAllString(string(out), ""), 200)
}

type Shortcut struct {
	Cmd  string `json:"cmd"`
	Desc string `json:"desc"`
	Key  string `json:"key,omitempty"`
}

type Agent struct {
	PaneID  string `json:"pane_id"`
	Command string `json:"command"`
	Cwd     string `json:"cwd"`
}

func (s *Server) handleWorkspaces(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"workspaces": s.CollectWorkspaces(r.URL.Query().Get("git") != "0")})
}

func (s *Server) CollectLiveness() []Workspace { return s.collectWorkspaces(false, true) }

func (s *Server) CollectWorkspaces(probeGit bool) []Workspace {
	return s.collectWorkspaces(probeGit, false)
}

func (s *Server) collectWorkspaces(probeGit, lite bool) []Workspace {
	if s.WorkspaceRoot == "" {
		return []Workspace{}
	}

	var known map[string]bool
	if s.cfg() != nil {
		known = make(map[string]bool, len(s.cfg().Repos))
		for n := range s.cfg().Repos {
			known[n] = true
		}
	}
	workspaces := scanWorkspaces(s.WorkspaceRoot, s.DefaultBranch, known, probeGit)
	agents := scanAgents()
	for i := range workspaces {
		ws := &workspaces[i]
		for j := range ws.Repos {
			repo := &ws.Repos[j]
			for _, a := range agents {
				if a.Cwd == repo.Path {
					repo.PaneID = a.PaneID
				}
			}
		}
		for _, a := range agents {
			if strings.HasPrefix(a.Cwd, ws.Path+string(filepath.Separator)) || a.Cwd == ws.Path {
				ws.Agents = append(ws.Agents, a)
			}
		}
	}

	if s.cfg() != nil {
		repoRank := make(map[string]int, len(s.cfg().RepoOrder))
		for idx, name := range s.cfg().RepoOrder {
			repoRank[name] = idx
		}
		rank := func(name string) int {
			if r, ok := repoRank[name]; ok {
				return r
			}
			return len(repoRank) + 1
		}
		for i := range workspaces {
			ws := &workspaces[i]
			sort.SliceStable(ws.Repos, func(a, b int) bool {
				ra, rb := rank(ws.Repos[a].Name), rank(ws.Repos[b].Name)
				if ra != rb {
					return ra < rb
				}
				return ws.Repos[a].Name < ws.Repos[b].Name
			})
			running, total := 0, 0
			for j := range ws.Repos {
				repo := &ws.Repos[j]
				cfgRepo, ok := s.cfg().Repos[repo.Name]
				if !ok {
					continue
				}
				repo.Alias = cfgRepo.Alias
				if repo.Alias == "" {
					repo.Alias = repo.Name
				}
				if !lite {
					for _, sc := range cfgRepo.EffectiveShortcuts() {
						repo.Shortcuts = append(repo.Shortcuts, Shortcut{Cmd: sc.Cmd, Desc: sc.Desc, Key: sc.Key})
					}
					repo.Setup = append(repo.Setup, cfgRepo.EffectiveSetup()...)
				}
				for _, svcName := range cfgRepo.ServiceOrder {
					svc := cfgRepo.Services[svcName]
					win := services.ServiceHolderName(s.cfg().Session, ws.Branch, repo.Name, svcName)
					isRunning := ptyhost.HolderAlive(win)
					wsKey := services.PortWsKey(ws.Branch)
					port := services.Port(s.WorkspaceRoot, wsKey, repo.Alias+"~"+svcName)
					entry := Service{
						Name: svcName, Running: isRunning, TmuxWindow: win, Port: port,
					}
					if !isRunning {
						entry.Crashed, entry.CrashLog = serviceCrash(win)
					}
					if !lite {
						svcEnv := services.ServiceEnvironment(s.WorkspaceRoot, ws.Branch, repo.Alias+"/"+svcName)
						if svcEnv == "" {
							svcEnv = "local"
						}
						entry.Env = svcEnv
						entry.Profiles = cfgRepo.EnvProfiles(svc)
						if svc != nil && len(svc.Modes) > 0 {
							entry.Mode = s.svcMode(repo.Name, svcName, svc)
							if entry.Mode == "" {
								entry.Mode = "default"
							}
							entry.Modes = svc.ModeNames()
						}
					}
					if agent := codeagent.LookupAgent(svcName); agent != nil {
						entry.AgentName = agent.Name
					}
					repo.Services = append(repo.Services, entry)
					total++
					if isRunning {
						running++
					}
				}
			}
			for _, svcName := range s.cfg().WsServiceOrder {
				win := services.WsServiceHolderName(s.cfg().Session, ws.Branch, svcName)
				isRunning := ptyhost.HolderAlive(win)
				entry := Service{Name: svcName, Running: isRunning, TmuxWindow: win}
				if agent := codeagent.LookupAgent(svcName); agent != nil {
					entry.AgentName = agent.Name
					if ptyhost.HolderAlive(services.WsServiceHolderName(s.cfg().Session, ws.Branch, "claude-raw")) {
						entry.AgentState = "idle"
					}
				}
				ws.WsServices = append(ws.WsServices, entry)
				if entry.AgentName == "" {
					total++
					if isRunning {
						running++
					}
				}
			}
			ws.Running = running
			ws.Total = total
		}
	}
	for i := range workspaces {
		if workspaces[i].Repos == nil {
			workspaces[i].Repos = []Repo{}
		}
		if workspaces[i].Agents == nil {
			workspaces[i].Agents = []Agent{}
		}
	}
	return workspaces
}

func scanWorkspaces(root, defaultBranch string, knownRepos map[string]bool, probeGit bool) []Workspace {
	skeletons := workspace.Scan(root, defaultBranch, knownRepos)
	out := make([]Workspace, 0, len(skeletons))
	for _, sk := range skeletons {
		ws := Workspace{Branch: sk.Branch, IsMain: sk.IsMain, Path: sk.Path}
		for _, r := range sk.Repos {
			ws.Repos = append(ws.Repos, Repo{Name: r.Name, Path: r.Path})
		}
		ws.DisplayName = services.LoadWorkspaceState(ws.Path).DisplayName
		out = append(out, ws)
	}

	if probeGit {
		var wg sync.WaitGroup
		sem := make(chan struct{}, runtime.NumCPU()*2)
		for wi := range out {
			for ri := range out[wi].Repos {
				wg.Add(1)
				go func(r *Repo) {
					defer wg.Done()
					sem <- struct{}{}
					defer func() { <-sem }()
					fillRepoInfo(r)
				}(&out[wi].Repos[ri])
			}
		}
		wg.Wait()
	}
	return out
}

func fillRepoInfo(r *Repo) {
	out := gitOutput(r.Path, "status", "--porcelain=v2", "--branch")
	if out == "" {
		return
	}
	dirty := 0
	for _, line := range strings.Split(out, "\n") {
		if line == "" {
			continue
		}
		if strings.HasPrefix(line, "# branch.head ") {
			if b := strings.TrimPrefix(line, "# branch.head "); b != "" && b != "(detached)" {
				r.Branch = b
			}
			continue
		}
		if line[0] == '#' {
			continue
		}
		dirty++
	}
	r.Dirty = dirty
}

func scanAgents() []Agent { return nil }

func gitOutput(dir string, args ...string) string {
	out, err := exec.Command("git", append([]string{"-C", dir}, args...)...).Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func isGitRepo(path string) bool { return workspace.IsGitRepo(path) }
