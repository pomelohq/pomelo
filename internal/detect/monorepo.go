package detect

import (
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

// DetectRepo detects every runnable app in a repo. For a monorepo (turbo / nx /
// pnpm|yarn|npm workspaces / lerna / go.work) it returns one StackFacts per
// member app (with Dir set); otherwise a single root-level stack.
func DetectRepo(root string) []StackFacts {
	rules := loadRules()
	members := monorepoMembers(root)
	var out []StackFacts
	// A repo can be an app at its root AND have workspace members (e.g. a Rails
	// app with a JS streaming service). Include the root only when it's a real
	// app (has a framework) — not when it's just a workspace-root config file.
	if len(members) > 0 {
		if rf, ok := detectWith(rules, root, root); ok && rf.Framework != "" {
			out = append(out, rf)
		}
	}
	for _, m := range members {
		// lockRoot=root: a member's lockfile (bun.lock, uv.lock) usually lives at
		// the repo root, not in the member dir.
		if f, ok := detectWith(rules, filepath.Join(root, m), root); ok {
			f.Dir = m
			out = append(out, f)
		}
	}
	if len(out) > 0 {
		return out
	}
	if f, ok := Detect(root); ok {
		return []StackFacts{f}
	}
	return nil
}

// monorepoMembers returns repo-relative dirs of workspace members that hold a
// detectable app. Empty when the repo is not a monorepo.
func monorepoMembers(root string) []string {
	globs := workspaceGlobs(root)
	if len(globs) == 0 {
		return nil
	}
	// Polyglot monorepos keep apps outside the JS workspace (e.g. a Python
	// backend/ next to a JS frontend/), so also scan conventional app dirs.
	// The "detectable" filter below drops anything that isn't a real app.
	globs = append(globs,
		"backend", "frontend", "api", "server", "web", "client", "worker", "admin",
		"apps/*", "services/*", "packages/*",
	)
	seen := map[string]bool{}
	var dirs []string
	for _, g := range globs {
		matches, _ := filepath.Glob(filepath.Join(root, g))
		for _, m := range matches {
			fi, err := os.Stat(m)
			if err != nil || !fi.IsDir() {
				continue
			}
			rel, err := filepath.Rel(root, m)
			if err != nil || rel == "." || seen[rel] {
				continue
			}
			if f, ok := Detect(m); ok {
				// A frameworkless JS package in a workspace is a library, not a
				// runnable service — skip it (keeps next/vite/nest/... apps only).
				if f.Language == "js" && f.Framework == "" {
					continue
				}
				seen[rel] = true
				dirs = append(dirs, rel)
			}
		}
	}
	sort.Strings(dirs)
	return dirs
}

// workspaceGlobs resolves member globs from whatever monorepo tool the repo uses.
func workspaceGlobs(root string) []string {
	if g := goWorkDirs(root); len(g) > 0 {
		return g
	}
	if g := javaModules(root); len(g) > 0 {
		return g
	}
	if g := pnpmWorkspaceGlobs(root); len(g) > 0 {
		return g
	}
	if g := pkgJSONWorkspaces(root); len(g) > 0 {
		return g
	}
	if g := lernaPackages(root); len(g) > 0 {
		return g
	}
	// turbo/nx declare a monorepo but read members from the underlying workspace;
	// when none was found above, fall back to the conventional layout.
	if exists(root, "turbo.json") || exists(root, "turbo.jsonc") {
		return []string{"apps/*", "packages/*"}
	}
	if exists(root, "nx.json") {
		return []string{"apps/*", "libs/*", "packages/*"}
	}
	return nil
}

func exists(root, name string) bool {
	_, err := os.Stat(filepath.Join(root, name))
	return err == nil
}

func pnpmWorkspaceGlobs(root string) []string {
	data, err := os.ReadFile(filepath.Join(root, "pnpm-workspace.yaml"))
	if err != nil {
		return nil
	}
	var ws struct {
		Packages []string `yaml:"packages"`
	}
	if yaml.Unmarshal(data, &ws) != nil {
		return nil
	}
	return cleanGlobs(ws.Packages)
}

func pkgJSONWorkspaces(root string) []string {
	data, err := os.ReadFile(filepath.Join(root, "package.json"))
	if err != nil {
		return nil
	}
	var pkg struct {
		Workspaces json.RawMessage `json:"workspaces"`
	}
	if json.Unmarshal(data, &pkg) != nil || len(pkg.Workspaces) == 0 {
		return nil
	}
	var arr []string
	if json.Unmarshal(pkg.Workspaces, &arr) == nil {
		return cleanGlobs(arr)
	}
	var obj struct {
		Packages []string `json:"packages"`
	}
	if json.Unmarshal(pkg.Workspaces, &obj) == nil {
		return cleanGlobs(obj.Packages)
	}
	return nil
}

func lernaPackages(root string) []string {
	data, err := os.ReadFile(filepath.Join(root, "lerna.json"))
	if err != nil {
		return nil
	}
	var l struct {
		Packages []string `json:"packages"`
	}
	if json.Unmarshal(data, &l) != nil {
		return nil
	}
	return cleanGlobs(l.Packages)
}

var (
	mavenModuleRe   = regexp.MustCompile(`<module>\s*([^<\s]+)\s*</module>`)
	gradleIncludeRe = regexp.MustCompile(`include\(?\s*['"]:?([^'"]+)['"]`)
)

// javaModules enumerates submodules of a multi-module Maven/Gradle build.
func javaModules(root string) []string {
	var dirs []string
	if data, err := os.ReadFile(filepath.Join(root, "pom.xml")); err == nil {
		for _, m := range mavenModuleRe.FindAllStringSubmatch(string(data), -1) {
			dirs = append(dirs, strings.TrimSpace(m[1]))
		}
	}
	for _, sf := range []string{"settings.gradle", "settings.gradle.kts"} {
		data, err := os.ReadFile(filepath.Join(root, sf))
		if err != nil {
			continue
		}
		for _, m := range gradleIncludeRe.FindAllStringSubmatch(string(data), -1) {
			d := strings.ReplaceAll(strings.TrimSpace(m[1]), ":", "/")
			dirs = append(dirs, strings.TrimPrefix(d, "/"))
		}
	}
	return cleanGlobs(dirs)
}

func goWorkDirs(root string) []string {
	data, err := os.ReadFile(filepath.Join(root, "go.work"))
	if err != nil {
		return nil
	}
	var dirs []string
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		line = strings.TrimPrefix(line, "use")
		line = strings.Trim(line, " (){}")
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "go ") || line == "use" {
			continue
		}
		dirs = append(dirs, strings.TrimPrefix(line, "./"))
	}
	return cleanGlobs(dirs)
}

func cleanGlobs(in []string) []string {
	var out []string
	for _, g := range in {
		g = strings.TrimSpace(g)
		if g == "" || strings.HasPrefix(g, "!") {
			continue
		}
		out = append(out, g)
	}
	return out
}
