package services

import (
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"
)

func TestWatchTreeFiresOnCreate(t *testing.T) {
	root := t.TempDir()
	fired := make(chan struct{}, 8)
	stop, err := WatchTree(root, func() {
		select {
		case fired <- struct{}{}:
		default:
		}
	})
	if err != nil {
		t.Fatalf("watch: %v", err)
	}
	defer stop()

	time.Sleep(300 * time.Millisecond)
	if err := os.WriteFile(filepath.Join(root, "new.txt"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	select {
	case <-fired:
	case <-time.After(5 * time.Second):
		t.Fatal("watcher did not fire on file create")
	}
}

func TestWatchTreeSeesNestedDirs(t *testing.T) {
	root := t.TempDir()
	nested := filepath.Join(root, "a", "b")
	if err := os.MkdirAll(nested, 0o755); err != nil {
		t.Fatal(err)
	}
	fired := make(chan struct{}, 8)
	stop, err := WatchTree(root, func() {
		select {
		case fired <- struct{}{}:
		default:
		}
	})
	if err != nil {
		t.Fatalf("watch: %v", err)
	}
	defer stop()

	time.Sleep(300 * time.Millisecond)
	if err := os.WriteFile(filepath.Join(nested, "deep.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	select {
	case <-fired:
	case <-time.After(5 * time.Second):
		t.Fatal("watcher did not fire for a nested path")
	}
}

func TestWatchTreeCoalescesBurst(t *testing.T) {
	root := t.TempDir()
	var calls int32
	stop, err := WatchTree(root, func() { atomic.AddInt32(&calls, 1) })
	if err != nil {
		t.Fatalf("watch: %v", err)
	}
	defer stop()

	time.Sleep(300 * time.Millisecond)
	for i := range 40 {
		name := filepath.Join(root, "f"+string(rune('a'+i%26))+string(rune('0'+i/26))+".txt")
		if err := os.WriteFile(name, []byte("x\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}

	time.Sleep(2 * time.Second)
	if n := atomic.LoadInt32(&calls); n == 0 {
		t.Fatal("watcher never fired for the burst")
	} else if n > 5 {
		t.Fatalf("burst of 40 writes produced %d callbacks, expected them coalesced", n)
	}
}

func TestFSWatchIgnoredPath(t *testing.T) {
	ignored := []string{
		"/ws/api/.git/index",
		"/ws/web/node_modules/.vite/deps.json",
		"/ws/.pom/network.json",
		"/ws/app/.ddata/Build/x.o",
	}
	for _, p := range ignored {
		if !FSWatchIgnoredPath(p) {
			t.Errorf("expected %q to be ignored", p)
		}
	}
	kept := []string{
		"/ws/api/main.go",
		"/ws/CLAUDE.md",
		"/ws/web/src/gitignore.ts",
		"/ws/api/.gitignore",
	}
	for _, p := range kept {
		if FSWatchIgnoredPath(p) {
			t.Errorf("expected %q to be watched", p)
		}
	}
}

func TestWatchTreeIgnoresNoisyDirs(t *testing.T) {
	root := t.TempDir()
	gitDir := filepath.Join(root, "api", ".git")
	if err := os.MkdirAll(gitDir, 0o755); err != nil {
		t.Fatal(err)
	}
	time.Sleep(time.Second)

	var calls int32
	stop, err := WatchTree(root, func() { atomic.AddInt32(&calls, 1) })
	if err != nil {
		t.Fatalf("watch: %v", err)
	}
	defer stop()

	time.Sleep(500 * time.Millisecond)
	for i := range 10 {
		name := filepath.Join(gitDir, "index"+string(rune('0'+i)))
		if err := os.WriteFile(name, []byte("x\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	time.Sleep(1500 * time.Millisecond)

	if n := atomic.LoadInt32(&calls); n != 0 {
		t.Fatalf(".git writes fired the watcher %d times, expected 0", n)
	}
}

func TestWatchTreeStopIsIdempotent(t *testing.T) {
	root := t.TempDir()
	stop, err := WatchTree(root, func() {})
	if err != nil {
		t.Fatalf("watch: %v", err)
	}
	stop()
	stop()
}
