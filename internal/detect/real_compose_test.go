package detect_test

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/detect"
)

// For every real docker-compose under POM_DETECT_REAL_ROOT: ParseCompose → Emit →
// Load + Validate. Opt-in (skips without the env var), so it never runs in CI.
func TestRealComposeEmit(t *testing.T) {
	root := os.Getenv("POM_DETECT_REAL_ROOT")
	if root == "" {
		t.Skip("set POM_DETECT_REAL_ROOT to a dir of cloned repos")
	}

	var composeDirs []string
	seen := map[string]bool{}
	filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			if d.Name() == "node_modules" || d.Name() == ".git" {
				return filepath.SkipDir
			}
			rel, _ := filepath.Rel(root, path)
			if strings.Count(rel, string(filepath.Separator)) >= 3 {
				return filepath.SkipDir
			}
			return nil
		}
		switch d.Name() {
		case "docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml":
			dir := filepath.Dir(path)
			if !seen[dir] {
				seen[dir] = true
				composeDirs = append(composeDirs, dir)
			}
		}
		return nil
	})
	sort.Strings(composeDirs)

	withShared := 0
	for _, dir := range composeDirs {
		svcs := detect.ParseCompose(dir)
		var shared []detect.ComposeService
		var kinds []string
		for _, s := range svcs {
			if s.Kind == detect.KindShared {
				shared = append(shared, s)
				kinds = append(kinds, s.Name+"="+s.Type+"/"+string(s.Strategy))
			}
		}
		rel, _ := filepath.Rel(root, dir)
		if len(shared) == 0 {
			continue
		}
		withShared++
		rd := detect.RepoDetection{Name: "app", Apps: detect.DetectRepo(dir), Shared: shared}
		yml, err := detect.Emit("proj", []detect.RepoDetection{rd})
		if err != nil {
			t.Errorf("%s: Emit: %v", rel, err)
			continue
		}
		p := filepath.Join(t.TempDir(), "pom.yml")
		if err := os.WriteFile(p, []byte(yml), 0o644); err != nil {
			t.Fatal(err)
		}
		cfg, err := config.Load(p)
		if err != nil {
			t.Errorf("%s: Load failed:\n%s\nerr: %v", rel, yml, err)
			continue
		}
		if err := cfg.Validate(); err != nil {
			t.Errorf("%s: Validate failed:\n%s\nerr: %v", rel, yml, err)
			continue
		}
		t.Logf("%-52s %s", rel, strings.Join(kinds, "  "))
	}
	t.Logf("compose dirs: %d, with shared services: %d", len(composeDirs), withShared)
	if withShared == 0 {
		t.Fatal("no compose file yielded a shared service — parser likely broken")
	}
}
