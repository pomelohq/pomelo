// Package diskcache persists small JSON blobs (PR/Jira lookups) under the state
// dir so a restart shows the last-known data instantly while the network refresh
// runs in the background.
package diskcache

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"

	"github.com/pomelohq/pomelo/internal/paths"
)

var mu sync.Mutex

func path(name string) string { return paths.StatePath(filepath.Join("cache", name+".json")) }

func Load[T any](name string) (T, bool) {
	var v T
	mu.Lock()
	defer mu.Unlock()
	data, err := os.ReadFile(path(name))
	if err != nil || len(data) == 0 {
		return v, false
	}
	if json.Unmarshal(data, &v) != nil {
		return v, false
	}
	return v, true
}

func Save[T any](name string, v T) {
	data, err := json.Marshal(v)
	if err != nil {
		return
	}
	mu.Lock()
	defer mu.Unlock()
	p := path(name)
	_ = os.MkdirAll(filepath.Dir(p), 0o755)
	_ = os.WriteFile(p, data, 0o644)
}
