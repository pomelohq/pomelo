// Package detect turns a repo on disk into StackFacts (language, framework,
// package manager, install/run commands, port) using declarative rules. Rules
// are data (embedded YAML + user/project dirs), so adding a stack is adding a
// file — the engine never changes.
package detect

import (
	"embed"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"

	"gopkg.in/yaml.v3"
)

//go:embed stacks/*.yaml
var builtinFS embed.FS

type ManifestMatch struct {
	File    string `yaml:"file"`
	Pattern string `yaml:"pattern"`
}

type Match struct {
	Markers     []string        `yaml:"markers"`      // all must exist
	AnyMarkers  []string        `yaml:"any_markers"`  // at least one must exist
	InManifest  []ManifestMatch `yaml:"in_manifest"`  // all must match
	AnyManifest []ManifestMatch `yaml:"any_manifest"` // at least one must match
}

type PMRule struct {
	Lockfile string `yaml:"lockfile"`
	PM       string `yaml:"pm"`
}

type RunRule struct {
	Kind    string `yaml:"kind"`     // server | worker
	Cmd     string `yaml:"cmd"`      // supports {{pm}} {{port}}
	WhenDep string `yaml:"when_dep"` // include only if this dep appears in a manifest
	WhenPM  string `yaml:"when_pm"`  // include only if the resolved package manager matches
}

type Rule struct {
	ID              string            `yaml:"id"`
	Language        string            `yaml:"language"`
	Framework       string            `yaml:"framework"`
	Priority        int               `yaml:"priority"`
	Detect          Match             `yaml:"detect"`
	PackageManagers []PMRule          `yaml:"package_managers"`
	DefaultPM       string            `yaml:"default_pm"`
	Install         map[string]string `yaml:"install"`
	Run             []RunRule         `yaml:"run"`
	Setup           []string          `yaml:"setup"`
	DefaultPort     int               `yaml:"default_port"`
	DepCache        string            `yaml:"dep_cache"` // global-shared | per-project
}

type ResolvedRun struct {
	Kind string
	Cmd  string
}

type StackFacts struct {
	Dir            string // repo-relative dir of this app ("" = repo root; set for monorepo members)
	RuleID         string
	Language       string
	Framework      string
	PackageManager string
	Install        string
	Run            []ResolvedRun
	Setup          []string
	Port           int
	DepCache       string
}

var manifestFiles = []string{
	"requirements.txt", "pyproject.toml", "Pipfile", "package.json",
	"go.mod", "Gemfile", "pom.xml", "build.gradle", "build.gradle.kts",
}

// Detect returns the best-matching stack for a repo, or false if none match.
func Detect(repoPath string) (StackFacts, bool) {
	return detectWith(loadRules(), repoPath, repoPath)
}

// detectWith matches rules against dir. lockRoot is where lockfiles may also live
// (the repo root for a monorepo member, whose lockfile sits above it, not in it).
func detectWith(rules []Rule, dir, lockRoot string) (StackFacts, bool) {
	var matched []Rule
	for _, r := range rules {
		if ruleMatches(dir, r.Detect) {
			matched = append(matched, r)
		}
	}
	if len(matched) == 0 {
		return StackFacts{}, false
	}
	// highest priority wins; tie-break on longer id (more specific).
	sort.SliceStable(matched, func(i, j int) bool {
		if matched[i].Priority != matched[j].Priority {
			return matched[i].Priority > matched[j].Priority
		}
		return len(matched[i].ID) > len(matched[j].ID)
	})
	win := matched[0]
	inherit(&win, rules)
	return resolve(dir, lockRoot, win), true
}

func ruleMatches(root string, m Match) bool {
	if len(m.Markers) == 0 && len(m.AnyMarkers) == 0 && len(m.InManifest) == 0 && len(m.AnyManifest) == 0 {
		return false
	}
	for _, mk := range m.Markers {
		if !hasFile(root, mk) {
			return false
		}
	}
	if len(m.AnyMarkers) > 0 {
		ok := false
		for _, mk := range m.AnyMarkers {
			if hasFile(root, mk) {
				ok = true
				break
			}
		}
		if !ok {
			return false
		}
	}
	for _, im := range m.InManifest {
		if !manifestHas(root, im) {
			return false
		}
	}
	if len(m.AnyManifest) > 0 {
		ok := false
		for _, im := range m.AnyManifest {
			if manifestHas(root, im) {
				ok = true
				break
			}
		}
		if !ok {
			return false
		}
	}
	return true
}

// hasFile matches a plain name, a glob (next.config.*), or a recursive **/name.
func hasFile(root, pattern string) bool {
	if strings.HasPrefix(pattern, "**/") {
		base := strings.TrimPrefix(pattern, "**/")
		return walkFind(root, base) != ""
	}
	if m, _ := filepath.Glob(filepath.Join(root, pattern)); len(m) > 0 {
		return true
	}
	return false
}

func walkFind(root, base string) string {
	found := ""
	_ = filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err != nil || found != "" {
			return nil
		}
		if d.IsDir() && (d.Name() == "node_modules" || d.Name() == ".git" || d.Name() == "vendor") {
			return filepath.SkipDir
		}
		if !d.IsDir() {
			if ok, _ := filepath.Match(base, d.Name()); ok {
				found = p
			}
		}
		return nil
	})
	return found
}

func manifestHas(root string, im ManifestMatch) bool {
	var path string
	if strings.HasPrefix(im.File, "**/") {
		path = walkFind(root, strings.TrimPrefix(im.File, "**/"))
	} else if m, _ := filepath.Glob(filepath.Join(root, im.File)); len(m) > 0 {
		path = m[0]
	}
	if path == "" {
		return false
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return false
	}
	if re, err := regexp.Compile(im.Pattern); err == nil {
		return re.Match(data)
	}
	return strings.Contains(string(data), im.Pattern)
}

// inherit copies package-manager + dep-cache from the base-language rule when a
// framework rule doesn't define its own — keeps the YAML DRY.
func inherit(win *Rule, rules []Rule) {
	if len(win.PackageManagers) > 0 || win.DefaultPM != "" {
		return
	}
	for _, r := range rules {
		if r.Language == win.Language && r.Framework == "" {
			win.PackageManagers = r.PackageManagers
			win.DefaultPM = r.DefaultPM
			if win.Install == nil {
				win.Install = r.Install
			}
			if win.DepCache == "" {
				win.DepCache = r.DepCache
			}
			return
		}
	}
}

func resolve(root, lockRoot string, r Rule) StackFacts {
	pm := r.DefaultPM
	for _, p := range r.PackageManagers {
		if hasFile(root, p.Lockfile) || (lockRoot != root && hasFile(lockRoot, p.Lockfile)) {
			pm = p.PM
			break
		}
	}
	repl := strings.NewReplacer("{{pm}}", pm, "{{port}}", strconv.Itoa(r.DefaultPort))
	f := StackFacts{
		RuleID:         r.ID,
		Language:       r.Language,
		Framework:      r.Framework,
		PackageManager: pm,
		Install:        repl.Replace(r.Install[pm]),
		Setup:          r.Setup,
		Port:           r.DefaultPort,
		DepCache:       r.DepCache,
	}
	for _, run := range r.Run {
		if run.WhenDep != "" && !depPresent(root, run.WhenDep) {
			continue
		}
		if run.WhenPM != "" && run.WhenPM != pm {
			continue
		}
		f.Run = append(f.Run, ResolvedRun{Kind: run.Kind, Cmd: repl.Replace(run.Cmd)})
	}
	return f
}

func depPresent(root, dep string) bool {
	for _, mf := range manifestFiles {
		if manifestHas(root, ManifestMatch{File: mf, Pattern: regexp.QuoteMeta(dep)}) {
			return true
		}
		if manifestHas(root, ManifestMatch{File: "**/" + mf, Pattern: regexp.QuoteMeta(dep)}) {
			return true
		}
	}
	return false
}

func loadRules() []Rule {
	var rules []Rule
	entries, _ := builtinFS.ReadDir("stacks")
	for _, e := range entries {
		data, err := builtinFS.ReadFile("stacks/" + e.Name())
		if err != nil {
			continue
		}
		var fileRules []Rule
		if yaml.Unmarshal(data, &fileRules) == nil {
			rules = append(rules, fileRules...)
		}
	}
	rules = append(rules, loadDir(userStacksDir())...)
	rules = append(rules, loadDir(".pom/stacks")...)
	return rules
}

func loadDir(dir string) []Rule {
	if dir == "" {
		return nil
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var out []Rule
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".yaml") {
			continue
		}
		data, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			continue
		}
		var fileRules []Rule
		if yaml.Unmarshal(data, &fileRules) == nil {
			out = append(out, fileRules...)
		}
	}
	return out
}

func userStacksDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".config", "pom", "stacks")
}
