package detect_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/pomelohq/pomelo/internal/detect"
)

func writeRepo(t *testing.T, files map[string]string) string {
	t.Helper()
	dir := t.TempDir()
	for name, content := range files {
		p := filepath.Join(dir, name)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

func hasRun(f detect.StackFacts, sub string) bool {
	for _, r := range f.Run {
		if strings.Contains(r.Cmd, sub) {
			return true
		}
	}
	return false
}

func hasWorker(f detect.StackFacts) bool {
	for _, r := range f.Run {
		if r.Kind == "worker" {
			return true
		}
	}
	return false
}

func mustDetect(t *testing.T, dir string) detect.StackFacts {
	t.Helper()
	f, ok := detect.Detect(dir)
	if !ok {
		t.Fatalf("expected a detected stack, got none")
	}
	return f
}

// ---------- Python ----------

func TestPython(t *testing.T) {
	t.Run("django+celery", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"manage.py":        "",
			"requirements.txt": "django\ncelery\n",
		}))
		if f.Language != "python" || f.Framework != "django" {
			t.Fatalf("got %s/%s", f.Language, f.Framework)
		}
		if f.PackageManager != "pip" || f.Port != 8000 {
			t.Fatalf("pm=%s port=%d", f.PackageManager, f.Port)
		}
		if !hasRun(f, "runserver") {
			t.Fatalf("no runserver: %+v", f.Run)
		}
		if !hasWorker(f) {
			t.Fatalf("celery worker missing: %+v", f.Run)
		}
	})
	t.Run("fastapi+uv", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"app.py":           "",
			"requirements.txt": "fastapi\nuvicorn\n",
			"uv.lock":          "",
		}))
		if f.Framework != "fastapi" || f.PackageManager != "uv" || f.Port != 8000 {
			t.Fatalf("got fw=%s pm=%s port=%d", f.Framework, f.PackageManager, f.Port)
		}
		if !hasRun(f, "uvicorn") {
			t.Fatalf("no uvicorn: %+v", f.Run)
		}
		if !strings.Contains(f.Install, "uv ") {
			t.Fatalf("install not uv: %q", f.Install)
		}
	})
	t.Run("flask", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"app.py":           "",
			"requirements.txt": "Flask\n",
		}))
		if f.Framework != "flask" || f.Port != 5000 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
	t.Run("base", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{"requirements.txt": "requests\n"}))
		if f.Language != "python" || f.Framework != "" || f.PackageManager != "pip" {
			t.Fatalf("got %s/%s pm=%s", f.Language, f.Framework, f.PackageManager)
		}
	})
}

// ---------- JS ----------

func TestJS(t *testing.T) {
	t.Run("next+npm", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"package.json":      `{"dependencies":{"next":"14"}}`,
			"next.config.js":    "",
			"package-lock.json": "",
		}))
		if f.Framework != "next" || f.PackageManager != "npm" || f.Port != 3000 {
			t.Fatalf("got fw=%s pm=%s port=%d", f.Framework, f.PackageManager, f.Port)
		}
		if !hasRun(f, "npm run dev") {
			t.Fatalf("run: %+v", f.Run)
		}
	})
	t.Run("next+pnpm", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"package.json":   `{"dependencies":{"next":"14"}}`,
			"next.config.js": "",
			"pnpm-lock.yaml": "",
		}))
		if f.PackageManager != "pnpm" || !hasRun(f, "pnpm run dev") {
			t.Fatalf("pm=%s run=%+v", f.PackageManager, f.Run)
		}
	})
	t.Run("vite", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"package.json":   "{}",
			"vite.config.ts": "",
		}))
		if f.Framework != "vite" || f.Port != 5173 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
	t.Run("vite.mts", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"package.json":    "{}",
			"vite.config.mts": "",
		}))
		if f.Framework != "vite" {
			t.Fatalf("vite.config.mts not detected: %+v", f)
		}
	})
	t.Run("nest", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"package.json":  "{}",
			"nest-cli.json": "",
		}))
		if f.Framework != "nest" || f.Port != 3000 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
	t.Run("angular", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"package.json": `{"dependencies":{"@angular/core":"18"}}`,
			"angular.json": "{}",
		}))
		if f.Framework != "angular" || f.Port != 4200 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
	t.Run("express", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"package.json": `{"dependencies":{"express":"4"}}`,
		}))
		if f.Framework != "express" || f.Port != 3000 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
	t.Run("remix", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"package.json": `{"dependencies":{"@remix-run/react":"2"}}`,
		}))
		if f.Framework != "remix" || f.Port != 3000 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
}

// ---------- Java ----------

func TestJava(t *testing.T) {
	t.Run("spring-maven", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"pom.xml": "<project>spring-boot-starter-web</project>",
		}))
		if f.Framework != "spring-boot" || f.PackageManager != "maven" || f.Port != 8080 {
			t.Fatalf("got fw=%s pm=%s port=%d", f.Framework, f.PackageManager, f.Port)
		}
		if !hasRun(f, "spring-boot:run") {
			t.Fatalf("run: %+v", f.Run)
		}
	})
	t.Run("spring-gradle", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"build.gradle": "org.springframework.boot",
		}))
		if f.PackageManager != "gradle" || !hasRun(f, "bootRun") {
			t.Fatalf("pm=%s run=%+v", f.PackageManager, f.Run)
		}
	})
	t.Run("quarkus", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"pom.xml": "<project>quarkus-maven-plugin</project>",
		}))
		if f.Framework != "quarkus" || !hasRun(f, "quarkus:dev") {
			t.Fatalf("fw=%s run=%+v", f.Framework, f.Run)
		}
	})
	t.Run("plain", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{"pom.xml": "<project></project>"}))
		if f.Language != "java" || f.Framework != "" || f.PackageManager != "maven" {
			t.Fatalf("got %s/%s pm=%s", f.Language, f.Framework, f.PackageManager)
		}
	})
}

// ---------- Go ----------

func TestGo(t *testing.T) {
	t.Run("gin", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"go.mod": "module x\n\nrequire github.com/gin-gonic/gin v1.9.1\n",
		}))
		if f.Framework != "gin" || f.Port != 8080 || !hasRun(f, "go run") {
			t.Fatalf("got fw=%s port=%d run=%+v", f.Framework, f.Port, f.Run)
		}
	})
	t.Run("echo", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"go.mod": "module x\n\nrequire github.com/labstack/echo/v4 v4.11.0\n",
		}))
		if f.Framework != "echo" || f.Port != 1323 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
	t.Run("fiber", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"go.mod": "module x\n\nrequire github.com/gofiber/fiber/v2 v2.52.0\n",
		}))
		if f.Framework != "fiber" || f.Port != 3000 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
	t.Run("plain", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{"go.mod": "module x\n"}))
		if f.Language != "go" || f.Framework != "" || !strings.Contains(f.Install, "go mod download") {
			t.Fatalf("got %s/%s install=%q", f.Language, f.Framework, f.Install)
		}
	})
}

// ---------- Ruby ----------

func TestRuby(t *testing.T) {
	t.Run("rails+sidekiq", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"Gemfile":   "gem 'rails'\ngem 'sidekiq'\n",
			"bin/rails": "",
		}))
		if f.Framework != "rails" || f.Port != 3000 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
		if !hasRun(f, "rails server") || !hasWorker(f) {
			t.Fatalf("run: %+v", f.Run)
		}
		joined := strings.Join(f.Setup, " ")
		if !strings.Contains(joined, "db:migrate") {
			t.Fatalf("setup: %+v", f.Setup)
		}
	})
	t.Run("sinatra", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{
			"Gemfile": "gem 'sinatra'\n",
		}))
		if f.Framework != "sinatra" || f.Port != 4567 {
			t.Fatalf("got fw=%s port=%d", f.Framework, f.Port)
		}
	})
	t.Run("base", func(t *testing.T) {
		f := mustDetect(t, writeRepo(t, map[string]string{"Gemfile": "gem 'rake'\n"}))
		if f.Language != "ruby" || f.Framework != "" || f.PackageManager != "bundler" {
			t.Fatalf("got %s/%s pm=%s", f.Language, f.Framework, f.PackageManager)
		}
	})
}

func TestNoMatch(t *testing.T) {
	if _, ok := detect.Detect(writeRepo(t, map[string]string{"README.md": "hi"})); ok {
		t.Fatalf("expected no match for a bare repo")
	}
}
