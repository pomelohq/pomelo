package detect_test

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/pomelohq/pomelo/internal/detect"
)

// TestRealCorpus runs detection over every immediate subdir of the directory in
// POM_DETECT_REAL_ROOT and logs what it found. It is an opt-in integration check
// against a corpus of real cloned repos (see detect-testdata/CLAUDE.md); it skips
// when the env var is unset, so it never runs in CI.
//
//	POM_DETECT_REAL_ROOT=/path/to/detect-testdata go test ./internal/detect -run TestRealCorpus -v
func TestRealCorpus(t *testing.T) {
	root := os.Getenv("POM_DETECT_REAL_ROOT")
	if root == "" {
		t.Skip("set POM_DETECT_REAL_ROOT to a dir of cloned repos")
	}
	entries, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, e := range entries {
		if e.IsDir() {
			names = append(names, e.Name())
		}
	}
	sort.Strings(names)
	for _, name := range names {
		facts := detect.DetectRepo(filepath.Join(root, name))
		if len(facts) == 0 {
			t.Logf("%-28s NO MATCH", name)
			continue
		}
		var parts []string
		for _, f := range facts {
			d := f.Dir
			if d == "" {
				d = "."
			}
			parts = append(parts, d+":"+f.Language+"/"+f.Framework+"/"+f.PackageManager)
		}
		t.Logf("%-28s %s", name, strings.Join(parts, "  "))
	}
}
