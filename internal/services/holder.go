package services

import (
	"os/exec"
	"strconv"
	"strings"
	"syscall"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/pombin"
	"github.com/pomelohq/pomelo/internal/ptyhost"
)

func ServiceHolderName(session, branch, repo, svc string) string {
	return "svc-" + BranchSafe(session) + "-" + BranchSafe(branch) + "-" + repo + "-" + svc
}

func WsServiceHolderName(session, branch, svc string) string {
	return "ws-" + BranchSafe(session) + "-" + BranchSafe(branch) + "-" + svc
}

func SpawnHolder(name, cwd string, cols, rows int, argv []string) error {
	return SpawnHolderEnv(name, cwd, cols, rows, argv, nil)
}

func SpawnHolderEnv(name, cwd string, cols, rows int, argv, env []string) error {
	if ptyhost.HolderAlive(name) {
		return nil
	}
	bin, err := pombin.Path()
	if err != nil {
		return err
	}
	args := []string{"pty", "run", name, "--cwd", cwd}
	if cols > 0 && rows > 0 {
		args = append(args, "--cols", strconv.Itoa(cols), "--rows", strconv.Itoa(rows))
	}
	args = append(args, "--")
	args = append(args, argv...)
	cmd := exec.Command(bin, args...)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	// Every holder (shortcut, terminal, service) gets the tool-augmented PATH so its
	// commands can find node/npm/etc. from version managers that only init in an
	// interactive .zshrc — matching what the agent and `pom run_in_env` already use.
	cmd.Env = append(EnvWithToolPath(), env...)
	return cmd.Start()
}

func ServiceRunning(session, branch, repo, svc string) bool {
	return ptyhost.HolderAlive(ServiceHolderName(session, branch, repo, svc))
}

func StopService(session, branch, repo, svc string) error {
	return ptyhost.KillHolder(ServiceHolderName(session, branch, repo, svc))
}

func sessionOwns(session, holderName string) bool {
	s := BranchSafe(session)
	return strings.HasPrefix(holderName, "svc-"+s+"-") || strings.HasPrefix(holderName, "ws-"+s+"-")
}

func SessionRunning(session string) bool {
	for _, h := range ptyhost.Holders() {
		if sessionOwns(session, h.Name) {
			return true
		}
	}
	return false
}

func StopWorkspaceHolders(cfg *config.Config, branch string, otherBranches []string) int {
	m := workspaceHolderMatcher(cfg, branch, otherBranches)
	n := 0
	for _, h := range ptyhost.Holders() {
		if m(h.Name) && ptyhost.KillHolder(h.Name) == nil {
			n++
		}
	}
	return n
}

func workspaceHolderMatcher(cfg *config.Config, branch string, otherBranches []string) func(string) bool {
	session := cfg.Session
	exact := map[string]struct{}{}
	for repo, dir := range cfg.Repos {
		for svc := range dir.Services {
			exact[ServiceHolderName(session, branch, repo, svc)] = struct{}{}
		}
	}
	for _, svc := range cfg.WsServiceOrder {
		exact[WsServiceHolderName(session, branch, svc)] = struct{}{}
	}
	exact[WsServiceHolderName(session, branch, "claude-raw")] = struct{}{}

	shPrefix := "sh-" + WsKey(branch, false) + "-"
	var childPrefixes []string
	for _, b := range otherBranches {
		if b == branch {
			continue
		}
		if cp := "sh-" + WsKey(b, false) + "-"; strings.HasPrefix(cp, shPrefix) {
			childPrefixes = append(childPrefixes, cp)
		}
	}

	return func(name string) bool {
		if _, ok := exact[name]; ok {
			return true
		}
		if !strings.HasPrefix(name, shPrefix) {
			return false
		}
		for _, cp := range childPrefixes {
			if strings.HasPrefix(name, cp) {
				return false
			}
		}
		return true
	}
}

func StopSession(session string) int {
	n := 0
	for _, h := range ptyhost.Holders() {
		if sessionOwns(session, h.Name) {
			if ptyhost.KillHolder(h.Name) == nil {
				n++
			}
		}
	}
	return n
}
