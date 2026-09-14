package commands

import (
	"fmt"
	"github.com/pomelohq/pomelo/internal/provider/shell"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/lock"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

func Start(cfg *config.Config, cfgPath, target string) error {
	pairs, err := cfg.ResolveServices(target)
	if err != nil {
		return err
	}
	pairs = orderByDeps(cfg, pairs, false)
	configDir := filepath.Dir(cfgPath)
	branch := cfg.GlobalDefaultBranch()
	wsKey := services.PortWsKey(branch)
	needsPort := false
	for _, pair := range pairs {
		if dir, ok := cfg.Repos[pair[0]]; ok {
			if svc, ok := dir.Services[pair[1]]; ok && svc.HasPort() {
				needsPort = true
				break
			}
		}
	}
	if needsPort {
		services.ClaimBlock(configDir, wsKey)
	}

	lock.EnsureDir()
	started, skipped := 0, 0

	for _, pair := range pairs {
		dirName, svcName := pair[0], pair[1]
		if services.ServiceRunning(cfg.Session, branch, dirName, svcName) {
			fmt.Fprintf(os.Stderr, "%swarning:%s '%s' is already running — skipping\n", Yellow, NC, svcName)
			skipped++
			continue
		}

		resolved, err := cfg.ResolveService(configDir, dirName, svcName)
		if err != nil {
			return err
		}

		dir := cfg.Repos[dirName]
		alias := dirName
		if dir != nil && dir.Alias != "" {
			alias = dir.Alias
		}
		svc := dir.Services[svcName]
		port := 0
		if svc != nil && svc.HasPort() {
			p, perr := services.PreflightPort(configDir, wsKey, alias+"~"+svcName)
			if perr != nil {
				fmt.Fprintf(os.Stderr, "%swarning:%s %s — skipping %s\n", Yellow, NC, perr, svcName)
				skipped++
				continue
			}
			port = p
		}

		services.RegenerateWorkspaceEnv(configDir, cfg, branch)

		var fullCmd strings.Builder
		if resolved.Env != "" {
			fullCmd.WriteString(resolved.Env + " ")
		}
		fmt.Fprintf(&fullCmd, "cd '%s'", resolved.WorkDir)
		if resolved.PreStart != "" {
			fullCmd.WriteString(" && " + resolved.PreStart)
		}
		if port > 0 {
			fmt.Fprintf(&fullCmd, " && export PORT=%d", port)
		}
		fullCmd.WriteString(" && " + resolved.Cmd)

		holder := services.ServiceHolderName(cfg.Session, branch, dirName, svcName)
		if svc != nil && svc.HasPort() {
			services.RegisterServiceHolder(wsKey, alias+"~"+svcName, holder)
		}
		if err := services.SpawnHolder(holder, resolved.WorkDir, 0, 0, shell.Login(fullCmd.String())); err != nil {
			fmt.Fprintf(os.Stderr, "%swarning:%s %s — skipping %s\n", Yellow, NC, err, svcName)
			skipped++
			continue
		}
		fmt.Printf("%s>>>%s started %s%s%s (%s%s%s)\n", Green, NC, Bold, svcName, NC, Dim, dirName, NC)
		started++
	}

	if started > 0 {
		fmt.Printf("\n%s%d service(s) started%s in session %s%s%s\n", Green, started, NC, Cyan, cfg.Session, NC)
		fmt.Printf("%sattach: pom pty attach <name>%s\n", Dim, NC)
	}
	if skipped > 0 {
		fmt.Printf("%s%d service(s) skipped (already running)%s\n", Yellow, skipped, NC)
	}
	return nil
}

func Stop(cfg *config.Config, cfgPath, target string) error {
	branch := cfg.GlobalDefaultBranch()

	if target == "" {
		if n := services.StopSession(cfg.Session); n > 0 {
			fmt.Printf("%s>>>%s stopped %d service(s) (session %s%s%s)\n", Green, NC, n, Cyan, cfg.Session, NC)
		} else {
			fmt.Printf("%s>>>%s no running session '%s'\n", Blue, NC, cfg.Session)
		}
		return nil
	}

	pairs, err := cfg.ResolveServices(target)
	if err != nil {
		return err
	}
	pairs = orderByDeps(cfg, pairs, true)
	stopped := 0
	for _, pair := range pairs {
		dirName, svcName := pair[0], pair[1]
		if services.ServiceRunning(cfg.Session, branch, dirName, svcName) {
			_ = services.StopService(cfg.Session, branch, dirName, svcName)
			fmt.Printf("%s>>>%s stopped %s%s%s\n", Green, NC, Bold, svcName, NC)
			stopped++
		} else {
			fmt.Fprintf(os.Stderr, "%swarning:%s '%s' is not running\n", Yellow, NC, svcName)
		}
	}
	fmt.Printf("%s%d service(s) stopped%s\n", Green, stopped, NC)
	return nil
}

func StatusGlobal() {
	projects := services.ListProjects()
	fmt.Printf("%sRegistered projects:%s\n", Bold, NC)
	for name, dir := range projects {
		fmt.Printf("  %s%s%s → %s\n", Cyan, name, NC, dir)
	}
	if len(projects) == 0 {
		fmt.Printf("  %sno projects registered%s\n", Dim, NC)
	}
}

func Restart(cfg *config.Config, cfgPath, target string) error {
	if err := Stop(cfg, cfgPath, target); err != nil {
		return err
	}
	return Start(cfg, cfgPath, target)
}

func Status(cfg *config.Config) {
	branch := cfg.GlobalDefaultBranch()
	if !services.SessionRunning(cfg.Session) {
		fmt.Printf("%sno active session '%s'%s\n", Dim, cfg.Session, NC)
		return
	}
	fmt.Printf("%sSession:%s %s%s%s\n\n", Bold, NC, Cyan, cfg.Session, NC)
	for _, dirName := range cfg.RepoOrder {
		dir := cfg.Repos[dirName]
		fmt.Printf("%s%s%s\n", Bold, dirName, NC)
		for _, svcName := range dir.ServiceOrder {
			if services.ServiceRunning(cfg.Session, branch, dirName, svcName) {
				fmt.Printf("  %s●%s %s\n", Green, NC, svcName)
			} else {
				fmt.Printf("  %s○ %s%s\n", Dim, svcName, NC)
			}
		}
	}
	fmt.Printf("\n%sattach: pom pty attach <name>%s\n", Dim, NC)
}

func Attach(cfg *config.Config, target string) error {
	branch := cfg.GlobalDefaultBranch()
	repo := repoForSvc(cfg, target)
	if repo == "" {
		return fmt.Errorf("unknown service '%s'", target)
	}
	holder := services.ServiceHolderName(cfg.Session, branch, repo, target)
	if !ptyhost.HolderAlive(holder) {
		return fmt.Errorf("service '%s' is not running", target)
	}
	bin, err := os.Executable()
	if err != nil {
		return err
	}
	cmd := exec.Command(bin, "pty", "attach", holder)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = os.Stdin, os.Stdout, os.Stderr
	return cmd.Run()
}

func Logs(cfg *config.Config, target string) error {
	branch := cfg.GlobalDefaultBranch()
	repo := repoForSvc(cfg, target)
	holder := services.ServiceHolderName(cfg.Session, branch, repo, target)
	if repo == "" || !ptyhost.HolderAlive(holder) {
		return fmt.Errorf("service '%s' is not running", target)
	}
	os.Stdout.Write(ptyhost.Snapshot(holder, 200_000_000))
	return nil
}

func repoForSvc(cfg *config.Config, svc string) string {
	for _, name := range cfg.RepoOrder {
		if dir := cfg.Repos[name]; dir != nil {
			if _, ok := dir.Services[svc]; ok {
				return name
			}
		}
	}
	return ""
}

func orderByDeps(cfg *config.Config, pairs [][2]string, reverse bool) [][2]string {
	byDir := make(map[string][]string)
	var dirOrder []string
	for _, pair := range pairs {
		d := pair[0]
		if _, ok := byDir[d]; !ok {
			dirOrder = append(dirOrder, d)
		}
		byDir[d] = append(byDir[d], pair[1])
	}

	var result [][2]string
	for _, dirName := range dirOrder {
		dir, ok := cfg.Repos[dirName]
		if !ok {
			for _, svc := range byDir[dirName] {
				result = append(result, [2]string{dirName, svc})
			}
			continue
		}
		graph := config.BuildDepGraph(dir)
		var ordered []string
		var err error
		if reverse {
			ordered, err = graph.StopOrder(byDir[dirName])
		} else {
			ordered, err = graph.StartOrder(byDir[dirName])
		}
		if err != nil {
			ordered = byDir[dirName]
		}
		for _, svc := range ordered {
			result = append(result, [2]string{dirName, svc})
		}
	}
	return result
}
