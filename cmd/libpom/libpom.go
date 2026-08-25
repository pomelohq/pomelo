//go:build cgo

package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"encoding/json"
	"io"
	"log"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
	"unsafe"

	"github.com/pomelohq/pomelo/internal/agent/claude"
	"github.com/pomelohq/pomelo/internal/commands"
	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/core"
	"github.com/pomelohq/pomelo/internal/logbuf"
	"github.com/pomelohq/pomelo/internal/mcp"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
	"github.com/pomelohq/pomelo/internal/sessions"
)

var (
	mu     sync.Mutex
	srv    *core.Server
	appCfg *config.Config
	appDir string
)

const appVersion = "0.1.0"

//export PomInit
func PomInit(cfgPath *C.char) *C.char {
	core.Version = appVersion
	logbuf.Setup()
	services.LoadLoginShellEnv()
	p := C.GoString(cfgPath)
	cfg, err := config.Load(p)
	if err != nil {
		log.Printf("app init failed: config load %s: %v", p, err)
		return C.CString("error: " + err.Error())
	}
	log.Printf("app init: session=%s dir=%s version=%s", cfg.Session, filepath.Dir(p), appVersion)
	dir := filepath.Dir(p)
	services.InitNetwork(dir, cfg.Session, cfg)
	services.SetSharedStable(cfg.Session)
	reg := sessions.Load()
	reg.Touch(cfg.Session, dir, time.Now().Unix())
	_ = reg.Save()
	s := core.New("", cfg.Session, dir, cfg.GlobalDefaultBranch(), cfg)
	s.StartApp()
	mu.Lock()
	srv = s
	appCfg, appDir = cfg, dir
	mu.Unlock()
	if err := claude.InstallGlobalClaudeHook(); err != nil {
		log.Printf("app init: install global claude hook: %v", err)
	} else {
		log.Printf("app init: global claude hook installed")
	}
	if err := claude.InstallGlobalClaudeMCP(); err != nil {
		log.Printf("app init: install global claude MCP: %v", err)
	} else {
		log.Printf("app init: global claude MCP registered")
	}
	return C.CString("ok:" + cfg.Session)
}

//export PomFree
func PomFree(p *C.char) { C.free(unsafe.Pointer(p)) }

//export PomRunSubcommand
func PomRunSubcommand(argvJSON *C.char) C.int {
	var a []string
	if json.Unmarshal([]byte(C.GoString(argvJSON)), &a) != nil || len(a) == 0 {
		return 2
	}
	switch a[0] {
	case "pty":
		if len(a) >= 2 && a[1] == "run" {
			if runPTYHolder(a[2:]) != nil {
				return 1
			}
			return 0
		}
	case "mcp":
		if runMCPServer(a[1:]) != nil {
			return 1
		}
		return 0
	case "claude-hook":
		runClaudeHook()
		return 0
	case "prepare-main":
		if runPrepareMain() != nil {
			return 1
		}
		return 0
	}
	return 2
}

func runClaudeHook() {
	raw, err := io.ReadAll(os.Stdin)
	if err != nil {
		return
	}
	_ = claude.WriteHookState(raw)
}

func runPrepareMain() error {
	services.LoadLoginShellEnv()
	path, err := config.FindConfig()
	if err != nil {
		return err
	}
	cfg, err := config.Load(path)
	if err != nil {
		return err
	}
	dir := filepath.Dir(path)
	services.InitNetwork(dir, cfg.Session, cfg)
	services.SetSharedStable(cfg.Session)
	return commands.PrepareMain(cfg, dir)
}

func runPTYHolder(a []string) error {
	if len(a) < 1 {
		return os.ErrInvalid
	}
	name, rest := a[0], a[1:]
	cwd, cols, rows := "", 0, 0
	var argv []string
	for i := 0; i < len(rest); i++ {
		switch rest[i] {
		case "--cwd":
			if i++; i < len(rest) {
				cwd = rest[i]
			}
		case "--cols":
			if i++; i < len(rest) {
				cols, _ = strconv.Atoi(rest[i])
			}
		case "--rows":
			if i++; i < len(rest) {
				rows, _ = strconv.Atoi(rest[i])
			}
		case "--":
			argv = rest[i+1:]
			i = len(rest)
		}
	}
	if cwd == "" {
		cwd, _ = os.Getwd()
	}
	s, ln, err := ptyhost.ListenAndServe(name, ptyhost.StartOpts{Argv: argv, Dir: cwd, Env: os.Environ(), Cols: cols, Rows: rows})
	if err != nil {
		return err
	}
	defer ln.Close()
	return s.Wait()
}

func runMCPServer(a []string) error {
	branch := ""
	for i := 0; i < len(a); i++ {
		if a[i] == "--branch" && i+1 < len(a) {
			i++
			branch = a[i]
		}
	}
	services.LoadLoginShellEnv()
	path, err := config.FindConfig()
	if err != nil {
		return mcp.Serve(os.Stdin, os.Stdout, "pomelo", "app", nil)
	}
	cfg, err := config.Load(path)
	if err != nil {
		return mcp.Serve(os.Stdin, os.Stdout, "pomelo", "app", nil)
	}
	if branch == "" {
		branch = mcpBranchFromCwd()
	}
	dir := filepath.Dir(path)
	services.InitNetwork(dir, cfg.Session, cfg)
	services.SetSharedStable(cfg.Session)
	h := core.New("", cfg.Session, dir, cfg.GlobalDefaultBranch(), cfg).Handler()
	return mcp.Serve(os.Stdin, os.Stdout, "pomelo", "app", mcp.ToolsHandler(h, branch))
}

func mcpBranchFromCwd() string {
	cwd, err := os.Getwd()
	if err != nil {
		return ""
	}
	for _, seg := range strings.Split(filepath.Clean(cwd), string(filepath.Separator)) {
		if b, ok := strings.CutPrefix(seg, "workspace--"); ok && b != "" {
			return b
		}
	}
	return ""
}

func main() {}
