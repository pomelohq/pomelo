package appstate

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"

	"github.com/pomelohq/pomelo/internal/paths"
)

type JiraConfig struct {
	Site     string `json:"site,omitempty"`
	Email    string `json:"email,omitempty"`
	TokenEnv string `json:"token_env,omitempty"`
}

type SyncConfig struct {
	Configured         bool `json:"configured"`
	RefreshMain        bool `json:"refresh_main"`
	RefreshIntervalSec int  `json:"refresh_interval_sec,omitempty"`
}

type RemoteConfig struct {
	Enabled bool   `json:"enabled"`
	Token   string `json:"token,omitempty"`
	Port    int    `json:"port,omitempty"`
}

type State struct {
	Jira   JiraConfig   `json:"jira"`
	Sync   SyncConfig   `json:"sync"`
	Remote RemoteConfig `json:"remote"`
}

var mu sync.Mutex

func path(session string) string {
	name := session
	if name == "" {
		name = "_"
	}
	return paths.StatePath(filepath.Join("integrations", name+".json"))
}

func Load(session string) State {
	mu.Lock()
	defer mu.Unlock()
	var s State
	if data, err := os.ReadFile(path(session)); err == nil {
		_ = json.Unmarshal(data, &s)
	}
	return s
}

func Save(session string, s State) error {
	mu.Lock()
	defer mu.Unlock()
	p := path(session)
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(p, append(data, '\n'), 0o644)
}
