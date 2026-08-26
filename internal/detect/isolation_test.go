package detect_test

import (
	"strings"
	"testing"

	"github.com/pomelohq/pomelo/internal/detect"
)

func TestStrategiesCount(t *testing.T) {
	if n := len(detect.Strategies()); n != 7 {
		t.Fatalf("expected 7 strategies, got %d", n)
	}
}

func TestDatabasePerBranch(t *testing.T) {
	a := detect.ResolveNamespace(detect.DatabasePerBranch, "proj", "feat/x", nil)
	b := detect.ResolveNamespace(detect.DatabasePerBranch, "proj", "feat/y", nil)
	if a.Value == b.Value {
		t.Fatalf("branches must get distinct databases: %q", a.Value)
	}
	if !strings.HasPrefix(a.Value, "proj_") {
		t.Fatalf("db name not session-prefixed: %q", a.Value)
	}
	if a.Shared {
		t.Fatalf("database-per-branch is not shared")
	}
}

func TestBucketPerBranch(t *testing.T) {
	a := detect.ResolveNamespace(detect.BucketPerBranch, "Proj", "Feat/X", nil)
	if a.Value != strings.ToLower(a.Value) {
		t.Fatalf("bucket must be dns/lowercase: %q", a.Value)
	}
	b := detect.ResolveNamespace(detect.BucketPerBranch, "Proj", "Feat/Y", nil)
	if a.Value == b.Value {
		t.Fatalf("buckets must differ per branch")
	}
}

func TestVhostPerBranch(t *testing.T) {
	a := detect.ResolveNamespace(detect.VhostPerBranch, "proj", "feat/x", nil)
	if !strings.Contains(a.Value, "proj") || !strings.Contains(a.Value, "feat") {
		t.Fatalf("vhost should carry session+branch: %q", a.Value)
	}
}

func TestNamespacePrefix(t *testing.T) {
	a := detect.ResolveNamespace(detect.NamespacePrefix, "proj", "feat/x", nil)
	if !strings.HasSuffix(a.Value, "_") {
		t.Fatalf("namespace prefix should end with _: %q", a.Value)
	}
	b := detect.ResolveNamespace(detect.NamespacePrefix, "proj", "feat/y", nil)
	if a.Value == b.Value {
		t.Fatalf("prefixes must differ per branch")
	}
}

func TestKeyPrefix(t *testing.T) {
	a := detect.ResolveNamespace(detect.KeyPrefix, "proj", "feat/x", nil)
	if !strings.HasSuffix(a.Value, ":") || !strings.Contains(a.Value, "feat") {
		t.Fatalf("key prefix malformed: %q", a.Value)
	}
}

func TestSharedStateless(t *testing.T) {
	a := detect.ResolveNamespace(detect.SharedStateless, "proj", "feat/x", nil)
	if !a.Shared {
		t.Fatalf("shared-stateless must be Shared")
	}
}

func TestDBIndexSlot(t *testing.T) {
	alloc := detect.NewSlotAllocator(3)
	seen := map[string]bool{}
	for _, br := range []string{"a", "b", "c"} {
		ns := detect.ResolveNamespace(detect.DBIndexSlot, "proj", br, alloc)
		if ns.Strategy != detect.DBIndexSlot {
			t.Fatalf("branch %s should get a slot, got %s", br, ns.Strategy)
		}
		if seen[ns.Value] {
			t.Fatalf("duplicate slot %q", ns.Value)
		}
		seen[ns.Value] = true
	}
	// same branch → stable slot
	again := detect.ResolveNamespace(detect.DBIndexSlot, "proj", "a", alloc)
	if again.Strategy != detect.DBIndexSlot {
		t.Fatalf("stable branch lost its slot")
	}
	// 4th distinct branch → cap exhausted → fallback to key-prefix + warning
	over := detect.ResolveNamespace(detect.DBIndexSlot, "proj", "d", alloc)
	if over.Strategy != detect.KeyPrefix {
		t.Fatalf("expected key-prefix fallback, got %s", over.Strategy)
	}
	if over.Warning == "" {
		t.Fatalf("expected a slot-exhaustion warning")
	}
}
