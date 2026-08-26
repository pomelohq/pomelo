package detect

import (
	"strconv"
	"strings"
)

// Strategy is how a shared backing service isolates data per branch while a
// single instance is shared across all branches.
type Strategy string

const (
	DatabasePerBranch Strategy = "database-per-branch" // postgres, mysql, mongo, clickhouse
	DBIndexSlot       Strategy = "dbindex-slot"        // redis (cap 16) → fallback key-prefix
	KeyPrefix         Strategy = "key-prefix"          // memcached, nats
	BucketPerBranch   Strategy = "bucket-per-branch"   // minio, s3
	VhostPerBranch    Strategy = "vhost-per-branch"    // rabbitmq
	NamespacePrefix   Strategy = "namespace-prefix"    // kafka topic, es/opensearch index
	SharedStateless   Strategy = "shared-stateless"    // mailhog, mailpit
)

// Strategies lists every supported isolation strategy.
func Strategies() []Strategy {
	return []Strategy{
		DatabasePerBranch, DBIndexSlot, KeyPrefix, BucketPerBranch,
		VhostPerBranch, NamespacePrefix, SharedStateless,
	}
}

type Namespace struct {
	Strategy Strategy // effective strategy (may differ from requested on fallback)
	Value    string   // resolved namespace: db name / bucket / vhost / prefix / slot index
	Shared   bool     // true = no per-branch isolation (one shared namespace)
	Warning  string   // non-empty when a limit was hit (e.g. redis slot exhaustion)
}

// SlotAllocator hands out capacity-limited numeric slots (redis DB indexes),
// stable per branch key. Allocate returns ok=false once the cap is exhausted.
type SlotAllocator struct {
	cap   int
	byKey map[string]int
	used  map[int]bool
}

func NewSlotAllocator(capN int) *SlotAllocator {
	return &SlotAllocator{cap: capN, byKey: map[string]int{}, used: map[int]bool{}}
}

func (a *SlotAllocator) Allocate(key string) (int, bool) {
	if s, ok := a.byKey[key]; ok {
		return s, true
	}
	for i := 0; i < a.cap; i++ {
		if !a.used[i] {
			a.used[i] = true
			a.byKey[key] = i
			return i, true
		}
	}
	return 0, false
}

// ResolveNamespace resolves the per-branch namespace for a strategy. alloc is
// only used by dbindex-slot; pass a shared allocator per service instance.
func ResolveNamespace(strat Strategy, session, branch string, alloc *SlotAllocator) Namespace {
	sb := safe(session) + "_" + safe(branch)
	switch strat {
	case DatabasePerBranch:
		return Namespace{Strategy: strat, Value: sb}
	case BucketPerBranch:
		return Namespace{Strategy: strat, Value: dnsSafe(session + "-" + branch)}
	case VhostPerBranch:
		return Namespace{Strategy: strat, Value: safe(session) + "/" + safe(branch)}
	case NamespacePrefix:
		return Namespace{Strategy: strat, Value: sb + "_"}
	case KeyPrefix:
		return Namespace{Strategy: strat, Value: safe(session) + ":" + safe(branch) + ":"}
	case DBIndexSlot:
		if alloc == nil {
			alloc = NewSlotAllocator(16)
		}
		if slot, ok := alloc.Allocate(session + "/" + branch); ok {
			return Namespace{Strategy: strat, Value: strconv.Itoa(slot)}
		}
		return Namespace{
			Strategy: KeyPrefix,
			Value:    safe(session) + ":" + safe(branch) + ":",
			Warning:  "redis DB-index slots exhausted (cap " + strconv.Itoa(alloc.cap) + "); fell back to key-prefix",
		}
	case SharedStateless:
		return Namespace{Strategy: strat, Shared: true}
	}
	return Namespace{Strategy: strat, Value: sb}
}

func safe(s string) string {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9':
			b.WriteRune(r)
		default:
			b.WriteByte('_')
		}
	}
	return strings.Trim(b.String(), "_")
}

func dnsSafe(s string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(s) {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9':
			b.WriteRune(r)
		default:
			b.WriteByte('-')
		}
	}
	return strings.Trim(b.String(), "-")
}
