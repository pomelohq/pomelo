package pipeline

import (
	"fmt"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/services"
)

type CreateContext struct {
	WorkspaceName string
	Branch        string
	Config        *config.Config
	ConfigDir     string
	Session       string
	UniqueDirs    []string
	DirPaths      []services.DirMapping
	DirBranches   []services.DirBranch
	SkipStages    map[int]bool
	SelectedDirs  []services.DirBranch
	Environment   string

	GitBranch string

	Progress func(string)
}

func (c *CreateContext) progress(msg string) {
	if c.Progress != nil {
		c.Progress(msg)
	}
}

type DeleteContext struct {
	Branch        string
	Config        *config.Config
	ConfigDir     string
	CleanupItems  []CleanupItem
	DBsToDrop     []DBDropItem
	SkipStages    map[int]bool
	OtherBranches []string
}

type CleanupItem struct {
	DirPath   string
	WtPath    string
	WtBranch  string
	PreDelete []string
}

type DBDropItem struct {
	Host     string
	Port     uint16
	DBName   string
	User     string
	Password string
}

func FromConfig(cfg *config.Config, configPath, wsName, branch string, skipStages map[int]bool) (*CreateContext, error) {
	workspaces := cfg.AllWorkspaces()
	entries, ok := workspaces[wsName]
	if !ok {
		return nil, fmt.Errorf("workspace '%s' not found", wsName)
	}

	var uniqueDirs []string
	seen := make(map[string]bool)
	for _, entry := range entries {
		d, _, err := cfg.FindServiceEntry(entry)
		if err != nil {
			continue
		}
		if !seen[d] {
			seen[d] = true
			uniqueDirs = append(uniqueDirs, d)
		}
	}
	if len(uniqueDirs) == 0 {
		return nil, fmt.Errorf("no dirs found in workspace '%s'", wsName)
	}

	configDir := filepath.Dir(configPath)
	defaultBranch := cfg.GlobalDefaultBranch()

	var dirPaths []services.DirMapping
	for _, d := range uniqueDirs {
		resolved := d
		if !filepath.IsAbs(d) {
			wsPath := filepath.Join(configDir, "workspace--"+defaultBranch, d)
			if isDir(wsPath) {
				resolved = wsPath
			} else {
				resolved = filepath.Join(configDir, d)
			}
		}
		dirPaths = append(dirPaths, services.DirMapping{Name: d, Path: resolved})
	}

	var dirBranches []services.DirBranch
	for _, d := range uniqueDirs {
		dirPath := ""
		for _, dp := range dirPaths {
			if dp.Name == d {
				dirPath = dp.Path
				break
			}
		}
		b := gitBranch(dirPath)
		if b == "" {
			b = "main"
		}
		dirBranches = append(dirBranches, services.DirBranch{Name: d, Branch: b})
	}

	return &CreateContext{
		WorkspaceName: wsName,
		Branch:        branch,
		Config:        cfg,
		ConfigDir:     configDir,
		Session:       cfg.Session,
		UniqueDirs:    uniqueDirs,
		DirPaths:      dirPaths,
		DirBranches:   dirBranches,
		SkipStages:    skipStages,
	}, nil
}

func FromConfigWithSelection(cfg *config.Config, configPath, wsName, branch string, selected []services.DirBranch) (*CreateContext, error) {
	ctx, err := FromConfig(cfg, configPath, wsName, branch, nil)
	if err != nil {
		return nil, err
	}

	selectedNames := make(map[string]bool)
	for _, s := range selected {
		selectedNames[s.Name] = true
	}

	var filteredDirs []string
	for _, d := range ctx.UniqueDirs {
		if selectedNames[d] {
			filteredDirs = append(filteredDirs, d)
		}
	}
	// Fail loudly instead of provisioning an empty 0/0 workspace: an empty (or
	// non-matching) selection would otherwise "succeed" with no worktrees.
	if len(filteredDirs) == 0 {
		return nil, fmt.Errorf("no repos selected for workspace '%s'", wsName)
	}
	ctx.UniqueDirs = filteredDirs

	var filteredPaths []services.DirMapping
	for _, dp := range ctx.DirPaths {
		if selectedNames[dp.Name] {
			filteredPaths = append(filteredPaths, dp)
		}
	}
	ctx.DirPaths = filteredPaths

	var filteredBranches []services.DirBranch
	for _, db := range ctx.DirBranches {
		if selectedNames[db.Name] {
			filteredBranches = append(filteredBranches, db)
		}
	}
	ctx.DirBranches = filteredBranches

	ctx.SelectedDirs = selected
	ctx.SkipStages = nil
	return ctx, nil
}

func gitBranch(dirPath string) string {
	out, err := exec.Command("git", "-C", dirPath, "rev-parse", "--abbrev-ref", "HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func isDir(path string) bool {
	return services.DirExists(path)
}
