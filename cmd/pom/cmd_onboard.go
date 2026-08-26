package main

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/core"
	"github.com/pomelohq/pomelo/internal/services"
	"github.com/pomelohq/pomelo/internal/sessions"
	"github.com/spf13/cobra"
)

var onboardCmd = &cobra.Command{
	Use:   "onboard [session]",
	Short: "Analyze repos and author a runnable pom.yml with an autonomous agent",
	Long: `Runs Pomelo's onboarding agent against a session. With --new it first
scaffolds the session (clones the --repo paths, writes a seed pom.yml), then the
agent analyzes every repo (framework, monorepo apps, processes, setup, shared
services from docker-compose incl. extends, repo aliases) and writes a correct
config, looping config_doctor until it's clean. Portless, single process.`,
	PersistentPreRunE: func(cmd *cobra.Command, args []string) error { return nil },
	RunE: func(cmd *cobra.Command, args []string) error {
		newName, _ := cmd.Flags().GetString("new")
		repoPaths, _ := cmd.Flags().GetStringArray("repo")
		branch, _ := cmd.Flags().GetString("branch")
		model, _ := cmd.Flags().GetString("model")
		if branch == "" {
			branch = "main"
		}

		var cfgPath string
		if newName != "" {
			if len(repoPaths) == 0 {
				return fmt.Errorf("--new requires at least one --repo")
			}
			req := core.CreateSessionReq{Name: newName, DefaultBranch: branch}
			for _, p := range repoPaths {
				abs, _ := filepath.Abs(p)
				req.Repos = append(req.Repos, core.RepoSpec{Path: abs})
			}
			fmt.Printf("Scaffolding session %q from %d repo(s)…\n", newName, len(repoPaths))
			dir, err := core.ScaffoldSession(req)
			if err != nil {
				return err
			}
			fmt.Printf("Seeded %s\n\n", filepath.Join(dir, "pom.yml"))
			cfgPath = filepath.Join(dir, "pom.yml")
		} else {
			name := ""
			if len(args) > 0 {
				name = args[0]
			}
			if name != "" {
				ss := sessions.Load().Get(name)
				if ss == nil {
					return fmt.Errorf("unknown session %q", name)
				}
				p, err := config.FindConfigFrom(ss.Path)
				if err != nil {
					return fmt.Errorf("no pom.yml in %s", ss.Path)
				}
				cfgPath = p
			} else {
				p, err := config.FindConfig()
				if err != nil {
					return err
				}
				cfgPath = p
			}
		}

		services.LoadLoginShellEnv()
		cfg, err := config.Load(cfgPath)
		if err != nil {
			return err
		}
		dir := filepath.Dir(cfgPath)
		services.InitNetwork(dir, cfg.Session, cfg)
		services.SetSharedStable(cfg.Session)
		srv := core.New("", cfg.Session, dir, cfg.GlobalDefaultBranch(), cfg)
		fmt.Printf("Onboarding %q — analyzing repos and authoring pom.yml…\n\n", cfg.Session)
		return srv.RunOnboardCLI(cfg.GlobalDefaultBranch(), true, model, os.Stdout)
	},
}

func init() {
	onboardCmd.Flags().String("new", "", "Scaffold a NEW session with this name before onboarding")
	onboardCmd.Flags().StringArray("repo", nil, "Local repo path (or git URL) to clone; repeatable (with --new)")
	onboardCmd.Flags().String("branch", "main", "Default branch for the new session")
	onboardCmd.Flags().String("model", "", "Claude model for the agent (default: sonnet)")
}
