package sessions

import (
	"encoding/json"
	"os"
	"path/filepath"

	"github.com/pomelohq/pomelo/internal/paths"
)

type Session struct {
	Name     string `json:"name"`
	Path     string `json:"path"`
	LastUsed int64  `json:"last_used"`
}

type Registry struct {
	Current  string    `json:"current"`
	Sessions []Session `json:"sessions"`
}

func SessionsRoot() string {
	if r := os.Getenv("POM_SESSIONS_ROOT"); r != "" {
		return r
	}
	home, _ := os.UserHomeDir()
	if home == "" {
		return "/tmp/pom"
	}
	return filepath.Join(home, "pom")
}

func isDir(p string) bool {
	st, err := os.Stat(p)
	return err == nil && st.IsDir()
}

func registryPath() string { return paths.StatePath("sessions.json") }

func Load() *Registry {
	r := &Registry{}
	data, err := os.ReadFile(registryPath())
	if err != nil {
		return r
	}
	_ = json.Unmarshal(data, r)
	return r
}

func (r *Registry) Save() error {
	paths.EnsureStateDir()
	data, err := json.MarshalIndent(r, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(registryPath(), data, 0o644)
}

func (r *Registry) Get(name string) *Session {
	for i := range r.Sessions {
		if r.Sessions[i].Name == name {
			return &r.Sessions[i]
		}
	}
	return nil
}

func (r *Registry) CurrentSession() *Session {
	if s := r.Get(r.Current); s != nil {
		return s
	}
	var best *Session
	for i := range r.Sessions {
		if best == nil || r.Sessions[i].LastUsed > best.LastUsed {
			best = &r.Sessions[i]
		}
	}
	return best
}

func (r *Registry) Touch(name, path string, now int64) {
	for i := range r.Sessions {
		if r.Sessions[i].Name == name {
			r.Sessions[i].Path = path
			r.Sessions[i].LastUsed = now
			r.Current = name
			return
		}
	}
	r.Sessions = append(r.Sessions, Session{Name: name, Path: path, LastUsed: now})
	r.Current = name
}

func (r *Registry) Remove(name string) {
	out := r.Sessions[:0]
	for _, s := range r.Sessions {
		if s.Name != name {
			out = append(out, s)
		}
	}
	r.Sessions = out
	if r.Current == name {
		r.Current = ""
	}
}
