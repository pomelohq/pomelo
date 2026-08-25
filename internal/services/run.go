package services

import (
	"context"
	"os"
	"os/exec"
	"time"
)

func RunTimeout(timeout time.Duration, dir, name string, args ...string) ([]byte, error) {
	return RunTimeoutEnv(timeout, dir, nil, name, args...)
}

func RunTimeoutEnv(timeout time.Duration, dir string, env []string, name string, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, name, args...)
	if dir != "" {
		cmd.Dir = dir
	}
	if len(env) > 0 {
		cmd.Env = append(os.Environ(), env...)
	}
	return cmd.Output()
}
