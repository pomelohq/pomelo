package detect_test

import (
	"testing"

	"github.com/pomelohq/pomelo/internal/detect"
)

func factByDir(fs []detect.StackFacts, dir string) (detect.StackFacts, bool) {
	for _, f := range fs {
		if f.Dir == dir {
			return f, true
		}
	}
	return detect.StackFacts{}, false
}

func TestMonorepoTurboPnpm(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"turbo.json":              "{}",
		"pnpm-workspace.yaml":     "packages:\n  - 'apps/*'\n",
		"package.json":            "{}",
		"apps/web/package.json":   `{"dependencies":{"next":"14"}}`,
		"apps/web/next.config.js": "",
		"apps/api/package.json":   `{"dependencies":{"express":"4"}}`,
	})
	facts := detect.DetectRepo(root)
	if len(facts) != 2 {
		t.Fatalf("expected 2 apps, got %d: %+v", len(facts), facts)
	}
	web, ok := factByDir(facts, "apps/web")
	if !ok || web.Framework != "next" {
		t.Fatalf("apps/web should be next: %+v", facts)
	}
	api, ok := factByDir(facts, "apps/api")
	if !ok || api.Framework != "express" {
		t.Fatalf("apps/api should be express: %+v", facts)
	}
}

// nx apps run via `nx serve <name>`; libraries and e2e projects are dropped.
// (Modelled on nx-examples, an Angular nx workspace.)
func TestMonorepoNxProjectJSON(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"nx.json":                     "{}",
		"package.json":                `{"dependencies":{"@angular/core":"21"}}`,
		"yarn.lock":                   "",
		"apps/cart/project.json":      `{"name":"cart","projectType":"application"}`,
		"apps/cart/package.json":      `{"name":"cart"}`,
		"apps/cart-e2e/project.json":  `{"name":"cart-e2e","projectType":"application","tags":["type:e2e"]}`,
		"libs/shared/ui/project.json": `{"name":"shared-ui","projectType":"library"}`,
	})
	facts := detect.DetectRepo(root)
	cart, ok := factByDir(facts, "apps/cart")
	if !ok || cart.Framework != "angular" {
		t.Fatalf("apps/cart should be angular: %+v", facts)
	}
	if len(cart.Run) == 0 || cart.Run[0].Cmd != "yarn nx serve cart" {
		t.Fatalf("apps/cart run should be 'yarn nx serve cart': %+v", cart.Run)
	}
	if _, ok := factByDir(facts, "apps/cart-e2e"); ok {
		t.Fatalf("e2e project should be dropped: %+v", facts)
	}
	if _, ok := factByDir(facts, "libs/shared/ui"); ok {
		t.Fatalf("library should be dropped: %+v", facts)
	}
}

// Single-app nx workspace: root project.json is the app, e2e sibling dropped.
// (Caught on node-express-realworld: root missed, e2e mis-detected.)
func TestMonorepoNxRootApp(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"nx.json":          "{}",
		"package.json":     `{"dependencies":{"express":"4"}}`,
		"project.json":     `{"name":"api","projectType":"application","targets":{"serve":{}}}`,
		"e2e/project.json": `{"name":"e2e","projectType":"application","targets":{"e2e":{}}}`,
		"e2e/package.json": `{"name":"e2e"}`,
	})
	facts := detect.DetectRepo(root)
	api, ok := factByDir(facts, "")
	if !ok || api.Framework != "express" {
		t.Fatalf("root should be express: %+v", facts)
	}
	if api.Run[0].Cmd != "npx nx serve api" {
		t.Fatalf("root run should be 'npx nx serve api': %+v", api.Run)
	}
	if _, ok := factByDir(facts, "e2e"); ok {
		t.Fatalf("e2e project should be dropped: %+v", facts)
	}
}

func TestMonorepoPkgJSONWorkspaces(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"package.json":               `{"workspaces":["packages/*"]}`,
		"packages/ui/package.json":   `{"dependencies":{"next":"14"}}`,
		"packages/ui/next.config.js": "",
	})
	facts := detect.DetectRepo(root)
	ui, ok := factByDir(facts, "packages/ui")
	if !ok || ui.Framework != "next" {
		t.Fatalf("packages/ui should be next: %+v", facts)
	}
}

func TestMonorepoGoWork(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"go.work":      "go 1.22\n\nuse ./svc-a\nuse ./svc-b\n",
		"svc-a/go.mod": "module a\n\nrequire github.com/gin-gonic/gin v1.9.1\n",
		"svc-b/go.mod": "module b\n\nrequire github.com/labstack/echo/v4 v4.11.0\n",
	})
	facts := detect.DetectRepo(root)
	if len(facts) != 2 {
		t.Fatalf("expected 2 go modules, got %d: %+v", len(facts), facts)
	}
	a, _ := factByDir(facts, "svc-a")
	b, _ := factByDir(facts, "svc-b")
	if a.Framework != "gin" || b.Framework != "echo" {
		t.Fatalf("svc-a=gin svc-b=echo, got %s/%s", a.Framework, b.Framework)
	}
}

// Polyglot monorepo: JS workspace (frontend) + a Python backend/ that is NOT a
// workspace member. Both must be detected. (Caught on full-stack-fastapi-template.)
func TestMonorepoPolyglot(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"package.json":            `{"workspaces":["frontend"]}`,
		"frontend/package.json":   "{}",
		"frontend/vite.config.ts": "",
		"backend/pyproject.toml":  "[project]\ndependencies = [\"fastapi\"]\n",
		"backend/app/main.py":     "",
	})
	facts := detect.DetectRepo(root)
	fe, ok := factByDir(facts, "frontend")
	if !ok || fe.Framework != "vite" {
		t.Fatalf("frontend should be vite: %+v", facts)
	}
	be, ok := factByDir(facts, "backend")
	if !ok || be.Framework != "fastapi" {
		t.Fatalf("backend should be fastapi: %+v", facts)
	}
}

// A repo that is an app at its root AND has JS workspace members (Rails app +
// a streaming Node service). Both must appear. (Caught on mastodon.)
func TestMonorepoRootAppPlusMember(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"Gemfile":                "gem 'rails'\ngem 'sidekiq'\n",
		"bin/rails":              "",
		"package.json":           `{"workspaces":["streaming"]}`,
		"streaming/package.json": `{"dependencies":{"express":"4"}}`,
	})
	facts := detect.DetectRepo(root)
	rootApp, ok := factByDir(facts, "")
	if !ok || rootApp.Framework != "rails" {
		t.Fatalf("root should be rails: %+v", facts)
	}
	st, ok := factByDir(facts, "streaming")
	if !ok || st.Framework != "express" {
		t.Fatalf("streaming should be express: %+v", facts)
	}
}

// A monorepo member's lockfile lives at the repo root, not in the member dir —
// PM must resolve from the root. (Caught on full-stack-fastapi-template: bun.lock
// at root, frontend detected as npm before the fix.)
func TestMonorepoLockAtRoot(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"package.json":            `{"workspaces":["frontend"]}`,
		"bun.lock":                "",
		"frontend/package.json":   `{"dependencies":{"next":"14"}}`,
		"frontend/next.config.js": "",
	})
	fe, ok := factByDir(detect.DetectRepo(root), "frontend")
	if !ok || fe.PackageManager != "bun" {
		t.Fatalf("frontend pm should be bun (root lock), got %+v", fe)
	}
}

// Multi-module Maven build: each <module> is a separate app. (Caught on
// quarkus-quickstarts, which was mis-detected as one spring-boot app at root.)
func TestMonorepoMavenModules(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"pom.xml":       "<project><modules><module>svc-a</module><module>svc-b</module></modules></project>",
		"svc-a/pom.xml": "<project>quarkus-maven-plugin</project>",
		"svc-b/pom.xml": "<project>spring-boot-starter-web</project>",
	})
	facts := detect.DetectRepo(root)
	a, _ := factByDir(facts, "svc-a")
	b, _ := factByDir(facts, "svc-b")
	if a.Framework != "quarkus" || b.Framework != "spring-boot" {
		t.Fatalf("svc-a=quarkus svc-b=spring-boot, got %s/%s (%+v)", a.Framework, b.Framework, facts)
	}
}

// A frameworkless JS package in a workspace is a library, not a service — it must
// be excluded. (Caught on astro/strapi: dozens of packages/* libs flooded output.)
func TestMonorepoDropsJSLibraries(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"package.json":                `{"workspaces":["packages/*"]}`,
		"packages/app/package.json":   `{"dependencies":{"next":"14"}}`,
		"packages/app/next.config.js": "",
		"packages/ui/package.json":    `{"name":"ui"}`,
	})
	facts := detect.DetectRepo(root)
	if _, ok := factByDir(facts, "packages/ui"); ok {
		t.Fatalf("library packages/ui should be excluded: %+v", facts)
	}
	if app, ok := factByDir(facts, "packages/app"); !ok || app.Framework != "next" {
		t.Fatalf("packages/app (next) should be kept: %+v", facts)
	}
}

func TestSingleRepoNotMonorepo(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"package.json":   `{"dependencies":{"next":"14"}}`,
		"next.config.js": "",
	})
	facts := detect.DetectRepo(root)
	if len(facts) != 1 || facts[0].Framework != "next" || facts[0].Dir != "" {
		t.Fatalf("expected 1 root next app, got %+v", facts)
	}
}
