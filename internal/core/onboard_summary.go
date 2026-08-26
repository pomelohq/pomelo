package core

import (
	"fmt"
	"io"
	"strings"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/doctor"
)

type OnboardCategory string

const (
	CatSecret OnboardCategory = "secret" // needs a credential from the user
	CatBoot   OnboardCategory = "boot"   // a service crashed on boot
	CatSetup  OnboardCategory = "setup"  // dependency install failed
	CatConfig OnboardCategory = "config" // config / tooling gap
)

// OnboardSummary groups the unresolved findings by what the user must do, so the
// UI can lead with "N need your input" (secrets) vs "N real errors" rather than a
// flat "onboarding failed".
type OnboardSummary struct {
	Runnable  bool
	Services  int
	BootOK    int
	Groups    map[OnboardCategory][]doctor.Finding
	CatsOrder []OnboardCategory
}

func classifyFinding(f doctor.Finding) OnboardCategory {
	switch {
	case strings.HasPrefix(f.ID, "secret."):
		return CatSecret
	case strings.HasPrefix(f.ID, "boot."):
		return CatBoot
	case strings.HasPrefix(f.ID, "setup."):
		return CatSetup
	default:
		return CatConfig
	}
}

func summarizeOnboard(cfg *config.Config, findings []doctor.Finding) OnboardSummary {
	total := 0
	if cfg != nil {
		for _, r := range cfg.Repos {
			total += len(r.Services)
		}
	}
	s := OnboardSummary{Runnable: len(findings) == 0, Services: total, Groups: map[OnboardCategory][]doctor.Finding{}}
	for _, f := range findings {
		c := classifyFinding(f)
		s.Groups[c] = append(s.Groups[c], f)
	}
	// Lead with what the user can act on (secrets), then boot, setup, config.
	for _, c := range []OnboardCategory{CatSecret, CatBoot, CatSetup, CatConfig} {
		if len(s.Groups[c]) > 0 {
			s.CatsOrder = append(s.CatsOrder, c)
		}
	}
	s.BootOK = total - len(s.Groups[CatBoot])
	if s.BootOK < 0 {
		s.BootOK = 0
	}
	return s
}

var catHeadline = map[OnboardCategory]string{
	CatSecret: "Needs a credential from you",
	CatBoot:   "Service didn't boot",
	CatSetup:  "Dependency install failed",
	CatConfig: "Config gap",
}

func (s OnboardSummary) render(out io.Writer) {
	if s.Runnable {
		fmt.Fprintln(out, "\nDone: config authored, deps installed, services booted - project is runnable.")
		return
	}
	n := 0
	for _, g := range s.Groups {
		n += len(g)
	}
	fmt.Fprintf(out, "\nConfig authored. Verified %d/%d services. %d item(s) need attention:\n", s.BootOK, s.Services, n)
	for _, c := range s.CatsOrder {
		fmt.Fprintf(out, "\n%s:\n", catHeadline[c])
		for _, f := range s.Groups[c] {
			fmt.Fprintf(out, "  - %s", f.Title)
			if f.Detail != "" {
				fmt.Fprintf(out, " — %s", f.Detail)
			}
			if f.Fix != "" {
				fmt.Fprintf(out, " (%s)", f.Fix)
			}
			fmt.Fprintln(out)
		}
	}
}
