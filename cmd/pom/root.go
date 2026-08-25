package main

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/spf13/cobra"
	_ "github.com/pomelohq/pomelo/internal/agent/codeagent"
	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/services"
)

const version = "0.1.5"

var (
	appConfig  *config.Config
	configPath string
)

var noConfigCmds = map[string]bool{
	"version": true, "completion": true, "help": true, "pom": true,
	"status": true, "disk": true, "doctor": true, "init": true,
	"setup": true, "ports": true, "pty": true, "ps": true, "url": true,
	"mcp": true, "onboard": true,
}

var rootCmd = &cobra.Command{
	Use:   "pom",
	Short: "Pomelo — per-branch dev environments for you and your agents",
	Long:  "Pomelo (pom) — run parallel per-ticket workspaces with real services, databases and ports, and drop AI agents into them.",
	PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
		top := cmd
		for top.Parent() != nil && top.Parent().Parent() != nil {
			top = top.Parent()
		}
		if noConfigCmds[top.Name()] {
			return nil
		}
		return loadConfig()
	},
	RunE: func(cmd *cobra.Command, args []string) error {
		return cmd.Help()
	},
	SilenceUsage:  true,
	SilenceErrors: true,
}

func loadConfig() error {
	checkDeps()
	var err error
	configPath, err = config.FindConfig()
	if err != nil {
		return err
	}
	appConfig, err = config.Load(configPath)
	if err != nil {
		return err
	}
	services.InitNetwork(filepath.Dir(configPath), appConfig.Session, appConfig)
	services.SetSharedStable(appConfig.Session)
	return nil
}

func checkDeps() {
	required := []struct{ name, install string }{
		{"docker", "https://docs.docker.com/get-docker/"},
		{"git", "brew install git"},
		{"zsh", "brew install zsh"},
	}
	for _, dep := range required {
		if _, err := exec.LookPath(dep.name); err != nil {
			fmt.Fprintf(os.Stderr, "required: %s not found — install: %s\n", dep.name, dep.install)
			os.Exit(1)
		}
	}
}

func configDir() string {
	return filepath.Dir(configPath)
}

func execute() {
	rootCmd.AddCommand(startCmd, stopCmd, restartCmd, statusCmd, refreshCmd)
	rootCmd.AddCommand(attachCmd, logsCmd, setupCmd)
	rootCmd.AddCommand(workspaceCmd, dbCmd, versionCmd, completionCmd, runCmd, diskCmd, urlCmd)
	rootCmd.AddCommand(mcpCmd, configCmd)
	rootCmd.AddCommand(releaseCmd, doctorCmd, initCmd)
	rootCmd.AddCommand(portsCmd, ptyCmd, psCmd, getCmd, describeCmd, applyCmd, envCmd)
	rootCmd.AddCommand(prepareMainCmd, onboardCmd)

	tryPluginDispatch()

	if err := rootCmd.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
