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
