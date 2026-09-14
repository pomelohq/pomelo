package core

import (
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

type captureSink struct {
	mu     sync.Mutex
	frames [][]byte
	closed bool
}

func (c *captureSink) SendJSON(v any) error { return nil }
func (c *captureSink) SendJSONBytes(b []byte) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.frames = append(c.frames, append([]byte(nil), b...))
	return nil
}
func (c *captureSink) SendText(b []byte) error   { return nil }
func (c *captureSink) SendBinary(b []byte) error { return nil }
func (c *captureSink) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.closed = true
	return nil
}
func (c *captureSink) count() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.frames)
}

func waitForFrames(t *testing.T, sink *captureSink, want int) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if sink.count() >= want {
			return
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatalf("expected at least %d frames, got %d", want, sink.count())
}

func TestOpenFilesStreamPushesInitialAndOnChange(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "api"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "api", "main.go"), []byte("package main\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	s := &Server{WorkspaceRoot: root}
	sink := &captureSink{}
	done := make(chan struct{})
	defer close(done)

	s.OpenFilesStream(sink, "main", true, done)
	waitForFrames(t, sink, 1)

	if err := os.WriteFile(filepath.Join(root, "api", "added.go"), []byte("package main\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	waitForFrames(t, sink, 2)
}

func TestOpenFilesStreamSkipsUnchangedListing(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "api"), 0o755); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(root, "api", "main.go")
	if err := os.WriteFile(target, []byte("package main\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	s := &Server{WorkspaceRoot: root}
	sink := &captureSink{}
	done := make(chan struct{})
	defer close(done)

	s.OpenFilesStream(sink, "main", true, done)
	waitForFrames(t, sink, 1)
	after := sink.count()

	if err := os.Chtimes(target, time.Now(), time.Now()); err != nil {
		t.Fatal(err)
	}
	time.Sleep(1500 * time.Millisecond)

	if n := sink.count(); n != after {
		t.Fatalf("touch with no listing change pushed %d extra frames", n-after)
	}
}
