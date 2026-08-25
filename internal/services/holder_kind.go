package services

import (
	"path/filepath"
	"strings"

	"github.com/pomelohq/pomelo/internal/config"
)

type HolderKind int

const (
	KindService   HolderKind = iota // svc-     managed repo service
	KindWsService                    // ws-      workspace service (incl claude-raw)
	KindTerminal                     // appsh-   ⌘T terminal tab
	KindShortcut                     // sh-      shortcut / editor shell
	KindRepoShell                    // reposh-  repo shell
	KindUnknown
)

// Holder answers the lifecycle questions that used to be scattered prefix checks
// (dialOrSpawnHolder, the ephemeral reaper, the activity display). A holder's kind
// is fully determined by its name, so HolderFor is the single place prefixes are parsed.
type Holder interface {
	Name() string
	Kind() HolderKind
	Reapable() bool          // the ephemeral-shell reaper may kill it
	AutoSpawnOnAttach() bool // attach spawns a bare zsh -i when no socket is up
	KeepOpen() bool          // hold the tab open after the command exits
	Display() (label, kind string)
}

func HolderFor(name string) Holder {
	switch {
	case strings.HasPrefix(name, "svc-"):
		return ServiceHolder{name, KindService}
	case strings.HasPrefix(name, "ws-"):
		return ServiceHolder{name, KindWsService}
	case strings.HasPrefix(name, "appsh-"):
		return ShellHolder{name, KindTerminal}
	case strings.HasPrefix(name, "sh-"):
		return ShellHolder{name, KindShortcut}
	case strings.HasPrefix(name, "reposh-"):
		return ShellHolder{name, KindRepoShell}
	}
	return ShellHolder{name, KindUnknown}
}

type ServiceHolder struct {
	name string
	kind HolderKind
}

func (h ServiceHolder) Name() string          { return h.name }
func (h ServiceHolder) Kind() HolderKind       { return h.kind }
func (ServiceHolder) Reapable() bool           { return false }
func (ServiceHolder) AutoSpawnOnAttach() bool  { return false }
func (ServiceHolder) KeepOpen() bool           { return false }
func (h ServiceHolder) Display() (string, string) { return h.name, "pty" }

type ShellHolder struct {
	name string
	kind HolderKind
}

func (h ShellHolder) Name() string    { return h.name }
func (h ShellHolder) Kind() HolderKind { return h.kind }

func (h ShellHolder) Reapable() bool {
	switch h.kind {
	case KindTerminal, KindShortcut, KindRepoShell:
		return true
	}
	return false
}

// A bare terminal tab (or an unrecognised name) is spawned on attach; pre-created
// shells (shortcut/editor/repo) are started elsewhere and only waited for.
func (h ShellHolder) AutoSpawnOnAttach() bool {
	return h.kind == KindTerminal || h.kind == KindUnknown
}

func (h ShellHolder) KeepOpen() bool { return h.kind == KindShortcut }

func (h ShellHolder) Display() (string, string) {
	switch h.kind {
	case KindTerminal:
		return "terminal", "shell"
	case KindShortcut:
		return "shortcut", "shortcut"
	case KindRepoShell:
		return "shell " + strings.TrimPrefix(h.name, "reposh-"), "shell"
	}
	return h.name, "pty"
}

// ResolveRepoEnv returns the repo-directory-wide env (no service merge) as KEY=VALUE
// pairs, resolved the same way ResolveServiceEnv resolves per-service env. Callers
// inject it via SpawnHolderEnv instead of sourcing a hardcoded .env.local file.
func ResolveRepoEnv(configDir string, cfg *config.Config, branch, dirName string) []string {
	dir := cfg.Repos[dirName]
	if dir == nil {
		return nil
	}
	alias := dir.Alias
	if alias == "" {
		alias = dirName
	}
	wsKey := PortWsKey(branch)
	wsState := LoadWorkspaceState(filepath.Join(configDir, "workspace--"+branch))
	envName := ""
	if wsState.ServiceEnvs != nil {
		if e, ok := wsState.ServiceEnvs[alias]; ok {
			envName = e
		}
	}
	dbNames := make(map[string]string, len(dir.Databases))
	for key, tpl := range dir.Databases {
		dbNames[key] = cfg.Session + "_" + ResolveBranchTokens(tpl, branch)
	}
	resolved := ResolveEnvTemplates(dir.Env, cfg, BranchSafe(branch), branch, wsKey, envName, dbNames)
	out := make([]string, 0, len(resolved))
	for _, e := range resolved {
		out = append(out, e.Key+"="+e.Value)
	}
	return out
}
