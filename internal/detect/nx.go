package detect

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// nxMembers detects apps in an nx workspace: deps are hoisted to the root and run
// targets are plugin-inferred, so members are read from project.json and run via
// `nx serve <name>` rather than the normal per-member rule engine.
func nxMembers(root string) []StackFacts {
	pm := nxPackageManager(root)
	dirs := nxProjects(root)
	// A single-app nx workspace puts project.json at the root; include it (Dir "").
	if exists(root, "project.json") {
		dirs = append(dirs, ".")
	}
	sort.Strings(dirs)

	var out []StackFacts
	for _, rel := range dirs {
		memberDir := filepath.Join(root, rel)
		var pj struct {
			Name        string              `json:"name"`
			ProjectType string              `json:"projectType"`
			Tags        []string            `json:"tags"`
			Targets     map[string]struct{} `json:"targets"`
		}
		data, err := os.ReadFile(filepath.Join(memberDir, "project.json"))
		if err != nil || json.Unmarshal(data, &pj) != nil {
			continue
		}
		if pj.ProjectType == "library" || nxIsE2E(pj.Name, rel, pj.Tags, pj.Targets) {
			continue
		}
		if !nxRunnable(pj.ProjectType, pj.Targets) {
			continue
		}
		name := pj.Name
		if name == "" {
			name = filepath.Base(rel)
		}
		dir := rel
		if dir == "." {
			dir = ""
		}
		fw, port := nxFramework(memberDir, root)
		out = append(out, StackFacts{
			Dir:            dir,
			RuleID:         "nx",
			Language:       "js",
			Framework:      fw,
			PackageManager: pm,
			Install:        pm + " install",
			Run:            []ResolvedRun{{Kind: "server", Cmd: nxServeCmd(pm, name)}},
			Port:           port,
			DepCache:       "per-project",
		})
	}
	return out
}

func nxRunnable(projectType string, targets map[string]struct{}) bool {
	if projectType == "application" {
		return true
	}
	for _, t := range []string{"serve", "dev", "start"} {
		if _, ok := targets[t]; ok {
			return true
		}
	}
	return false
}

func nxIsE2E(name, rel string, tags []string, targets map[string]struct{}) bool {
	base := filepath.Base(rel)
	if name == "e2e" || base == "e2e" || strings.HasSuffix(name, "-e2e") || strings.HasSuffix(base, "-e2e") {
		return true
	}
	if _, ok := targets["e2e"]; ok {
		return true
	}
	for _, t := range tags {
		if t == "type:e2e" {
			return true
		}
	}
	return false
}

func nxFramework(memberDir, root string) (string, int) {
	if globExists(memberDir, "next.config.*") {
		return "next", 3000
	}
	if hasFile(memberDir, "nest-cli.json") {
		return "nest", 3000
	}
	// Member deps first, then the root's (nx hoists them up).
	for _, dir := range []string{memberDir, root} {
		switch {
		case rootDep(dir, "@nestjs/core"):
			return "nest", 3000
		case rootDep(dir, "@angular/core"):
			return "angular", 4200
		case rootDep(dir, "next"):
			return "next", 3000
		case rootDep(dir, "express"):
			return "express", 3000
		case rootDep(dir, "fastify"):
			return "fastify", 3000
		}
	}
	if globExists(memberDir, "vite.config.*") {
		return "vite", 5173
	}
	return "", 0
}

func nxServeCmd(pm, name string) string {
	switch pm {
	case "pnpm":
		return "pnpm exec nx serve " + name
	case "yarn":
		return "yarn nx serve " + name
	case "bun":
		return "bunx nx serve " + name
	default:
		return "npx nx serve " + name
	}
}

func nxPackageManager(root string) string {
	for _, c := range []struct{ file, pm string }{
		{"pnpm-lock.yaml", "pnpm"}, {"yarn.lock", "yarn"},
		{"bun.lock", "bun"}, {"bun.lockb", "bun"}, {"package-lock.json", "npm"},
	} {
		if exists(root, c.file) {
			return c.pm
		}
	}
	return "npm"
}

func rootDep(root, dep string) bool {
	data, err := os.ReadFile(filepath.Join(root, "package.json"))
	if err != nil {
		return false
	}
	var pkg struct {
		Dependencies    map[string]string `json:"dependencies"`
		DevDependencies map[string]string `json:"devDependencies"`
	}
	if json.Unmarshal(data, &pkg) != nil {
		return false
	}
	if _, ok := pkg.Dependencies[dep]; ok {
		return true
	}
	_, ok := pkg.DevDependencies[dep]
	return ok
}

func globExists(dir, pattern string) bool {
	m, _ := filepath.Glob(filepath.Join(dir, pattern))
	return len(m) > 0
}
