package forge

import (
	"encoding/json"
	"sync"
	"time"

	"github.com/pomelohq/pomelo/internal/diskcache"
)

// Disk-backed cache for the heavy PR conversation reads (detail + review comments)
// so a relaunch shows the last-known content instantly and only refreshes in the
// background. Keyed by worktree path with a "detail:"/"comments:" prefix.

type prConvBlob struct {
	Body json.RawMessage `json:"body"`
	At   time.Time       `json:"at"`
}

const prConvTTL = 90 * time.Second

var (
	prConvMu   sync.Mutex
	prConv     = map[string]prConvBlob{}
	prConvOnce sync.Once
	prConvWarm sync.Mutex
)

func prConvHydrate() {
	prConvOnce.Do(func() {
		m, ok := diskcache.Load[map[string]prConvBlob]("pr_conv")
		if !ok {
			return
		}
		prConvMu.Lock()
		for k, v := range m {
			if _, exists := prConv[k]; !exists {
				prConv[k] = v
			}
		}
		prConvMu.Unlock()
	})
}

func prConvSave() {
	prConvMu.Lock()
	m := make(map[string]prConvBlob, len(prConv))
	for k, v := range prConv {
		m[k] = v
	}
	prConvMu.Unlock()
	diskcache.Save("pr_conv", m)
}

func prConvGet(key string) (body json.RawMessage, ok, stale bool) {
	prConvHydrate()
	prConvMu.Lock()
	defer prConvMu.Unlock()
	e, ok := prConv[key]
	if !ok {
		return nil, false, true
	}
	return e.Body, true, time.Since(e.At) >= prConvTTL
}

// InvalidateAll marks every PR cache (list head, detail, comments) stale so the
// next read refetches from origin. Backs the manual "Refresh" button.
func (s *Feature) InvalidateAll() {
	prHeadMu.Lock()
	prNextRetry = time.Time{}
	for k, e := range prHeadCache {
		e.at = time.Time{}
		prHeadCache[k] = e
	}
	prHeadMu.Unlock()

	prCacheMu.Lock()
	prCache = map[string]prCacheEntry{}
	prCacheMu.Unlock()

	prConvMu.Lock()
	for k, v := range prConv {
		v.At = time.Time{}
		prConv[k] = v
	}
	prConvMu.Unlock()
}

func prConvPut(key string, body json.RawMessage) {
	prConvMu.Lock()
	prConv[key] = prConvBlob{Body: body, At: time.Now()}
	prConvMu.Unlock()
	prConvSave()
}

// cachedConv serves the cached body immediately, refreshing in the background when
// stale. A cold key blocks once on the fetch.
func cachedConv(key string, fetch func() []byte) []byte {
	if body, ok, stale := prConvGet(key); ok {
		if stale {
			go func() {
				prConvWarm.Lock()
				defer prConvWarm.Unlock()
				if _, _, s := prConvGet(key); !s {
					return
				}
				prConvPut(key, fetch())
			}()
		}
		return body
	}
	body := fetch()
	prConvPut(key, body)
	return body
}
