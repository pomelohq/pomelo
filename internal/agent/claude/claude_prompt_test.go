package claude

import (
	"os"
	"path/filepath"
	"testing"
)

// The native installer puts claude in ~/.local/bin, which a GUI app's minimal PATH
// and a non-interactive login shell both miss. ResolveClaudeBin must still find it.
func TestResolveClaudeBinLocalBin(t *testing.T) {
	home := t.TempDir()
	bin := filepath.Join(home, ".local", "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	claude := filepath.Join(bin, "claude")
	if err := os.WriteFile(claude, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	t.Setenv("PATH", "/usr/bin:/bin") // claude not on PATH
	t.Setenv("SHELL", "/bin/zsh")

	claudeBinMu.Lock()
	claudeBinPath = ""
	claudeBinMu.Unlock()

	if got := ResolveClaudeBin(); got != claude {
		t.Fatalf("want %s, got %s", claude, got)
	}
}
