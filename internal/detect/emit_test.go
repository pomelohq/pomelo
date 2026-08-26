package detect_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/detect"
)

// emitAndLoad renders a draft and loads+validates it as a real config.
func emitAndLoad(t *testing.T, session string, repos []detect.RepoDetection) (*config.Config, string) {
	t.Helper()
	yml, err := detect.Emit(session, repos)
	if err != nil {
		t.Fatalf("Emit: %v", err)
	}
	dir := t.TempDir()
	path := filepath.Join(dir, "pom.yml")
	if err := os.WriteFile(path, []byte(yml), 0o644); err != nil {
		t.Fatal(err)
	}
	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("emitted config failed to load:\n%s\nerr: %v", yml, err)
	}
	if err := cfg.Validate(); err != nil {
		t.Fatalf("emitted config failed to validate:\n%s\nerr: %v", yml, err)
	}
	return cfg, yml
}

func TestEmitPolyglot(t *testing.T) {
	repos := []detect.RepoDetection{
		{
			Name: "api",
			Apps: []detect.StackFacts{{
				Language: "go", Framework: "gin", Install: "go mod download", Port: 8080,
				Run: []detect.ResolvedRun{{Kind: "server", Cmd: "go run ."}},
			}},
			Shared: []detect.ComposeService{
				{Name: "db", Kind: detect.KindShared, Type: "postgres", Strategy: detect.DatabasePerBranch},
				{Name: "cache", Kind: detect.KindShared, Type: "redis", Strategy: detect.DBIndexSlot},
				{Name: "web", Kind: detect.KindApp}, // must be ignored (app, not shared)
			},
		},
		{
			Name: "frontend",
			Apps: []detect.StackFacts{{
				Dir: "apps/web", Language: "js", Framework: "next", Install: "npm install", Port: 3000,
				Run: []detect.ResolvedRun{{Kind: "server", Cmd: "npm run dev"}},
			}},
		},
	}
	cfg, yml := emitAndLoad(t, "proj", repos)

	if len(cfg.Repos) != 2 {
		t.Fatalf("want 2 repos, got %d:\n%s", len(cfg.Repos), yml)
	}
	api := cfg.Repos["api"]
	if api == nil || api.Services["gin"] == nil || api.Services["gin"].Cmd != "go run ." {
		t.Fatalf("api/gin service missing:\n%s", yml)
	}
	if len(api.SharedSvcRefs) != 2 {
		t.Fatalf("api should reference 2 shared services, got %d:\n%s", len(api.SharedSvcRefs), yml)
	}
	fe := cfg.Repos["frontend"]
	if fe == nil || fe.Services["web"] == nil || fe.Services["web"].Dir != "apps/web" {
		t.Fatalf("frontend/web service (dir apps/web) missing:\n%s", yml)
	}
	if cfg.SharedServices["postgres"] == nil || cfg.SharedServices["redis"] == nil {
		t.Fatalf("top-level shared_services missing:\n%s", yml)
	}
	if strings.Contains(yml, "custom") {
		t.Fatalf("custom shared service should not be emitted:\n%s", yml)
	}
}

func TestEmitWorker(t *testing.T) {
	repos := []detect.RepoDetection{{
		Name: "app",
		Apps: []detect.StackFacts{{
			Language: "ruby", Framework: "rails", Install: "bundle install", Port: 3000,
			Setup: []string{"bin/rails db:migrate"},
			Run: []detect.ResolvedRun{
				{Kind: "server", Cmd: "bin/rails server -p 3000"},
				{Kind: "worker", Cmd: "bundle exec sidekiq"},
			},
		}},
	}}
	cfg, yml := emitAndLoad(t, "proj", repos)
	app := cfg.Repos["app"]
	if app.Services["rails"] == nil || app.Services["rails-worker"] == nil {
		t.Fatalf("want rails + rails-worker services:\n%s", yml)
	}
	joined := strings.Join(app.Setup, " ")
	if !strings.Contains(joined, "bundle install") || !strings.Contains(joined, "db:migrate") {
		t.Fatalf("setup missing install/migrate: %v", app.Setup)
	}
}
