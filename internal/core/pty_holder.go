package core

import (
	"fmt"
	"net"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/pomelohq/pomelo/internal/pombin"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

var ptySpawnMu sync.Mutex

func (s *Server) ptyCwd(wsKey string) string {
	if wsKey == "" {
		return s.WorkspaceRoot
	}
	isMain := strings.HasPrefix(wsKey, "main:")
	branch := strings.TrimPrefix(strings.TrimPrefix(wsKey, "main:"), "ws:")
	if branch == "" {
		return s.WorkspaceRoot
	}
	return services.WorkspaceRootDir(s.WorkspaceRoot, branch, isMain)
}

func (s *Server) dialOrSpawnHolder(name string, cols, rows int, cwd string) (net.Conn, error) {
	sockPath := ptyhost.SocketPath(name)
	if c, err := net.Dial("unix", sockPath); err == nil {
		return c, nil
	}
	// pre-created holders (services + shortcut/editor shells) are started elsewhere — attach just waits for their socket, never spawns a bare shell over the running command
	switch {
	case strings.HasPrefix(name, "svc-"), strings.HasPrefix(name, "ws-"),
		strings.HasPrefix(name, "sh-"), strings.HasPrefix(name, "reposh-"):
		if c, err := ptyhost.DialWait(sockPath, 4*time.Second); err == nil {
			return c, nil
		}
		return nil, fmt.Errorf("holder %q is not running", name)
	}
	ptySpawnMu.Lock()
	defer ptySpawnMu.Unlock()
	if c, err := net.Dial("unix", sockPath); err == nil {
		return c, nil
	}
	if err := s.spawnPtyHolder(name, cols, rows, cwd); err != nil {
		return nil, fmt.Errorf("cannot start pty holder: %w", err)
	}
	c, err := ptyhost.DialWait(sockPath, 5*time.Second)
	if err != nil {
		return nil, fmt.Errorf("pty holder did not come up: %w", err)
	}
	return c, nil
}

func (s *Server) spawnPtyHolder(name string, cols, rows int, cwd string) error {
	bin, err := pombin.Path()
	if err != nil {
		return err
	}
	if cwd == "" {
		cwd = s.WorkspaceRoot
	}
	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "zsh"
	}
	args := []string{"pty", "run", name, "--cwd", cwd}
	if cols > 0 && rows > 0 {
		args = append(args, "--cols", strconv.Itoa(cols), "--rows", strconv.Itoa(rows))
	}
	args = append(args, "--", shell, "-i")

	cmd := exec.Command(bin, args...)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	return cmd.Start()
}
