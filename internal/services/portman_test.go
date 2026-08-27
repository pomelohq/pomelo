package services

import (
	"fmt"
	"math/rand"
	"sync"
	"testing"
	"time"
)

func testManager(session string) *portManager {
	m := &portManager{
		session:  session,
		cmds:     make(chan portCmd, 64),
		leases:   map[string]*PortLease{},
		rng:      rand.New(rand.NewSource(1)),
		isUp:     func(int) bool { return false },
		bindable: func(int) bool { return true },
		now:      time.Now,
	}
	empty := map[string]int{}
	m.snap.Store(&empty)
	go m.loop()
	return m
}

func leaseState(m *portManager, key string) PortState {
	for _, l := range m.Snapshot() {
		if l.Key == key {
			return l.State
		}
	}
	return ""
}

func TestPortManagerConcurrentAcquireDistinct(t *testing.T) {
	m := testManager("sess-concurrent")

	const n = 10
	var wg sync.WaitGroup
	ports := make([]int, n)
	for i := 0; i < n; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			ports[i] = m.Acquire(fmt.Sprintf("ws-a\x1fsvc%d", i))
		}(i)
	}
	wg.Wait()

	seen := map[int]bool{}
	for i, p := range ports {
		if p < portLo || p > portHi {
			t.Fatalf("svc%d got out-of-range port %d", i, p)
		}
		if seen[p] {
			t.Fatalf("duplicate port %d handed to two concurrent services", p)
		}
		seen[p] = true
	}
	if len(seen) != n {
		t.Fatalf("want %d distinct ports, got %d", n, len(seen))
	}
}

func TestPortManagerStickyAndReadLockFree(t *testing.T) {
	m := testManager("sess-sticky")
	key := "ws-b\x1fapi"
	first := m.Acquire(key)

	var wg sync.WaitGroup
	for i := 0; i < 20; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if got := m.Acquire(key); got != first {
				t.Errorf("sticky broken: got %d want %d", got, first)
			}
			_ = m.portOf(key)
		}()
	}
	wg.Wait()

	if got := m.portOf(key); got != first {
		t.Fatalf("snapshot port %d != acquired %d", got, first)
	}
}

func TestPortManagerReapReleasesWhenDown(t *testing.T) {
	m := testManager("sess-reap")
	up := map[int]bool{}
	var mu sync.Mutex
	m.isUp = func(p int) bool { mu.Lock(); defer mu.Unlock(); return up[p] }
	clock := time.Now()
	m.now = func() time.Time { return clock }

	key := "ws-c\x1fweb"
	p := m.Acquire(key)

	mu.Lock()
	up[p] = true
	mu.Unlock()
	m.Reap()
	if s := leaseState(m, key); s != PortRunning {
		t.Fatalf("after up+reap state=%q want running", s)
	}

	// A single probe miss (a dev server blip) must NOT reclaim the port.
	mu.Lock()
	up[p] = false
	mu.Unlock()
	m.Reap()
	if m.portOf(key) == 0 {
		t.Fatalf("port reclaimed on a single down probe — a blip should survive")
	}

	// Only sustained unreachability past the grace window reclaims it.
	clock = clock.Add(reapDownGrace + time.Second)
	m.Reap()
	if m.portOf(key) != 0 {
		t.Fatalf("port still leased after sustained down")
	}
}

func TestPortManagerCrossProcessNoCollision(t *testing.T) {
	a := testManager("proc-a")
	b := testManager("proc-b")

	const n = 15
	var wg sync.WaitGroup
	ports := make([]int, 2*n)
	for i := 0; i < n; i++ {
		wg.Add(2)
		go func(i int) { defer wg.Done(); ports[i] = a.Acquire(fmt.Sprintf("ws\x1fa%d", i)) }(i)
		go func(i int) { defer wg.Done(); ports[n+i] = b.Acquire(fmt.Sprintf("ws\x1fb%d", i)) }(i)
	}
	wg.Wait()

	seen := map[int]bool{}
	for _, p := range ports {
		if p == 0 {
			t.Fatalf("pool exhausted unexpectedly")
		}
		if seen[p] {
			t.Fatalf("two processes handed out the same port %d", p)
		}
		seen[p] = true
	}
}

func TestPortManagerHydrateSurvivesRestart(t *testing.T) {
	m1 := testManager("sess-restart")
	key := "ws\x1fapi"
	p := m1.Acquire(key)
	m1.Mark(key, PortRunning)
	m1.Snapshot()

	m2 := newPortManager("sess-restart")
	if got := m2.portOf(key); got != p {
		t.Fatalf("hydrate lost lease: got %d want %d", got, p)
	}
	if s := leaseState(m2, key); s != PortRunning {
		t.Fatalf("hydrate lost state: got %q want running", s)
	}
}

func TestPortManagerFailedStartFreedAfterGrace(t *testing.T) {
	m := testManager("sess-grace")
	key := "ws-d\x1fnever"
	m.Acquire(key)
	m.Mark(key, PortStarting)

	m.cmds <- funcCmd(func() {
		m.leases[key].Since = m.leases[key].Since.Add(-2 * assignGrace)
	})
	m.Reap()
	if m.portOf(key) != 0 {
		t.Fatalf("starting-but-never-bound lease not freed after grace")
	}
}

func TestPortManagerAssignedReservationKept(t *testing.T) {
	m := testManager("sess-reserve")
	key := "ws-e\x1fenv"
	p := m.Acquire(key)

	m.cmds <- funcCmd(func() { m.leases[key].Since = m.leases[key].Since.Add(-10 * assignGrace) })
	m.Reap()
	if m.portOf(key) != p {
		t.Fatalf("assigned reservation was wrongly reaped")
	}
}
