package pipeline

import (
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/services"
)

func TestFromConfigWithSelectionRejectsEmpty(t *testing.T) {
	cfg := &config.Config{
		Session:   "myapp",
		RepoOrder: []string{"api"},
		Repos: map[string]*config.Dir{
			"api": {Alias: "api", ServiceOrder: []string{"server"}, Services: map[string]*config.Service{"server": {Cmd: "go run ."}}},
		},
	}

	if _, err := FromConfigWithSelection(cfg, "/tmp/pom.yml", "myapp", "feat", nil); err == nil {
		t.Error("empty selection must error, not provision an empty 0/0 workspace")
	}
	if _, err := FromConfigWithSelection(cfg, "/tmp/pom.yml", "myapp", "feat", []services.DirBranch{{Name: "ghost"}}); err == nil {
		t.Error("a selection matching no configured repo must error")
	}
	if _, err := FromConfigWithSelection(cfg, "/tmp/pom.yml", "myapp", "feat", []services.DirBranch{{Name: "api"}}); err != nil {
		t.Errorf("a valid selection must succeed: %v", err)
	}
}

func TestFromConfigIncludesServiceLessRepo(t *testing.T) {
	cfg := &config.Config{
		Session:   "myapp",
		RepoOrder: []string{"api", "infra"},
		Repos: map[string]*config.Dir{
			"api":   {Alias: "api", ServiceOrder: []string{"server"}, Services: map[string]*config.Service{"server": {Cmd: "go run ."}}},
			"infra": {Alias: "infra"},
		},
	}

	ctx, err := FromConfig(cfg, "/tmp/pom.yml", "myapp", "feat", nil)
	if err != nil {
		t.Fatalf("FromConfig: %v", err)
	}
	var hasInfra bool
	for _, d := range ctx.UniqueDirs {
		if d == "infra" {
			hasInfra = true
		}
	}
	if !hasInfra {
		t.Errorf("a service-less repo must still be a workspace member, got dirs %v", ctx.UniqueDirs)
	}

	if _, err := FromConfigWithSelection(cfg, "/tmp/pom.yml", "myapp", "feat", []services.DirBranch{{Name: "infra"}}); err != nil {
		t.Errorf("adding a service-less repo must succeed: %v", err)
	}
}

func TestFromConfigExplicitWorkspaceSkipsServiceLessRepo(t *testing.T) {
	cfg := &config.Config{
		Session:    "myapp",
		RepoOrder:  []string{"api", "infra"},
		Workspaces: map[string][]string{"myapp": {"api/server"}},
		Repos: map[string]*config.Dir{
			"api":   {Alias: "api", ServiceOrder: []string{"server"}, Services: map[string]*config.Service{"server": {Cmd: "go run ."}}},
			"infra": {Alias: "infra"},
		},
	}

	ctx, err := FromConfig(cfg, "/tmp/pom.yml", "myapp", "feat", nil)
	if err != nil {
		t.Fatalf("FromConfig: %v", err)
	}
	for _, d := range ctx.UniqueDirs {
		if d == "infra" {
			t.Errorf("an explicit (curated) workspace must not auto-include infra, got %v", ctx.UniqueDirs)
		}
	}
}
