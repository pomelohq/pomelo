package core

import (
	"strings"
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/doctor"
)

func TestSummarizeOnboardCategorizes(t *testing.T) {
	cfg := &config.Config{Repos: map[string]*config.Dir{
		"api": {Services: map[string]*config.Service{"web": {}, "worker": {}}},
	}}
	findings := []doctor.Finding{
		{ID: "secret.missing:STRIPE_KEY", Title: "Missing secret STRIPE_KEY"},
		{ID: "boot.api/web", Title: "service api/web failed to boot", Detail: "exited"},
		{ID: "setup.api", Title: "setup failed for api"},
		{ID: "shared.unwired:redis", Title: "redis unwired"},
	}
	s := summarizeOnboard(cfg, findings)

	if s.Runnable {
		t.Fatal("should not be runnable with findings")
	}
	if s.Services != 2 || s.BootOK != 1 { // 2 services, 1 boot failure -> 1 ok
		t.Fatalf("services=%d bootOK=%d", s.Services, s.BootOK)
	}
	if len(s.Groups[CatSecret]) != 1 || len(s.Groups[CatBoot]) != 1 ||
		len(s.Groups[CatSetup]) != 1 || len(s.Groups[CatConfig]) != 1 {
		t.Fatalf("bad grouping: %+v", s.Groups)
	}
	// Secrets lead the order (the user can act on them).
	if len(s.CatsOrder) == 0 || s.CatsOrder[0] != CatSecret {
		t.Fatalf("secrets should lead, got %v", s.CatsOrder)
	}
}

func TestSummarizeOnboardRunnable(t *testing.T) {
	var out strings.Builder
	summarizeOnboard(&config.Config{}, nil).render(&out)
	if !strings.Contains(out.String(), "runnable") {
		t.Fatalf("expected runnable message, got %q", out.String())
	}
}
