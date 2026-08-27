package services

import (
	"fmt"
	"strings"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/secrets"
	"github.com/pomelohq/pomelo/internal/tmpl"
)

type ResolveCtx struct {
	Cfg     *config.Config
	Branch  string
	WsKey   string
	EnvName string
	DBNames map[string]string
}

func ResolveTokens(s string, c ResolveCtx) string {
	return tmpl.Resolve(s, c.lookup, nil)
}

func (c ResolveCtx) lookup(key string) (string, bool) {
	parts := strings.Split(key, ".")
	head := parts[0]
	field := func(i int) string {
		if len(parts) > i {
			return parts[i]
		}
		return ""
	}
	switch head {
	case "bind_ip":
		return BindIP(), true
	case "branch":
		switch field(1) {
		case "":
			return c.Branch, true
		case "safe":
			return BranchSafe(c.Branch), true
		case "host":
			return BranchHost(c.Branch), true
		case "hash":
			return BranchHash(c.Branch), true
		}
		return "", false
	case "slot":
		if len(parts) < 2 {
			return "", false
		}
		return c.slotIndex(parts[1]), true
	case "db":
		if len(parts) < 2 {
			return "", false
		}
		return c.resolveDB(parts[1], field(2))
	case "secret":
		if len(parts) < 2 {
			return "", false
		}
		return secrets.Get(c.Cfg.Session, parts[1])
	case "shared":
		if len(parts) < 2 {
			return "", false
		}
		return c.resolveShared(parts[1], field(2))
	default:
		if len(parts) < 2 {
			return "", false
		}
		return c.resolveService(head, parts[1], field(2))
	}
}

func (c ResolveCtx) slotIndex(name string) string {
	allocs := LoadSlotAllocations()
	if svc, ok := allocs[name]; ok {
		if a, ok := svc.Slots[c.WsKey]; ok {
			return fmt.Sprintf("%d", a.Slot)
		}
	}
	return "0"
}

func (c ResolveCtx) sharedInstance(name string) int {
	if svc, ok := LoadSlotAllocations()[name]; ok {
		if a, ok := svc.Slots[c.WsKey]; ok {
			return a.Instance
		}
	}
	return 0
}

func (c ResolveCtx) resolveDB(name, field string) (string, bool) {
	dbName := c.DBNames[name]
	if field != "url" {
		return dbName, true
	}
	if pg := c.sharedName("postgres"); pg != "" {
		if conn, ok := c.resolveShared(pg, "url"); ok {
			return fmt.Sprintf("postgres://%s/%s", conn, dbName), true
		}
	}
	return dbName, true
}

func (c ResolveCtx) resolveShared(name, field string) (string, bool) {
	def, ok := c.Cfg.SharedServices[name]
	if !ok {
		return "", false
	}
	if field == "" {
		field = "url"
	}
	user, pass := def.DBUser, def.DBPassword
	if user == "" {
		user = "postgres"
	}
	if pass == "" {
		pass = "postgres"
	}
	// A capacity>1 shared service runs one container per instance at base+instance
	// (see AllocateSlot / GenerateSharedCompose). Resolve the port of THIS
	// workspace's assigned instance, not always instance 0.
	port := SharedPort(name) + c.sharedInstance(name)
	switch field {
	case "host":
		return "localhost", true
	case "port":
		return fmt.Sprintf("%d", port), true
	case "user":
		return user, true
	case "pass":
		return pass, true
	case "slot":
		return c.slotIndex(name), true
	case "url":
		if port == 0 {
			port = 5432
		}
		return fmt.Sprintf("%s:%s@localhost:%d", user, pass, port), true
	}
	return "", false
}

func (c ResolveCtx) resolveService(repo, svc, field string) (string, bool) {
	dir, alias := c.findRepo(repo)
	if dir == nil {
		return "", false
	}
	if field == "" {
		field = "url"
	}
	if field == "path" {
		return "/_pom_dev/" + repo + "/" + svc, true
	}
	if v, ok := c.envOverride(repo + "." + svc); ok {
		if field == "ws" {
			return httpToWs(v), true
		}
		return v, true
	}
	switch field {
	case "port":
		return fmt.Sprintf("%d", findRepoServicePort(c.Cfg, alias+"/"+svc, c.WsKey)), true
	case "host":
		return fmt.Sprintf("%s.%s.%s.%s", svc, alias, WorkspaceLabel(c.Branch), c.domain()), true
	case "url", "ws":
		u := fmt.Sprintf("http://%s.%s.%s.%s:%d", svc, alias, WorkspaceLabel(c.Branch), c.domain(), proxyPort(c.Cfg))
		if field == "ws" {
			return httpToWs(u), true
		}
		return u, true
	}
	return "", false
}

func (c ResolveCtx) envOverride(key string) (string, bool) {
	if c.Cfg.Environments == nil || c.EnvName == "" || c.EnvName == "local" {
		return "", false
	}
	if m, ok := c.Cfg.Environments[c.EnvName]; ok {
		if v, ok := m[key]; ok && v != "" {
			return v, true
		}
	}
	return "", false
}

func (c ResolveCtx) findRepo(name string) (*config.Dir, string) {
	for dirName, dir := range c.Cfg.Repos {
		alias := dir.Alias
		if alias == "" {
			alias = dirName
		}
		if dirName == name || alias == name {
			return dir, alias
		}
	}
	return nil, ""
}

func (c ResolveCtx) sharedName(kind string) string {
	for name, def := range c.Cfg.SharedServices {
		t := def.Type
		if t == "" {
			t = name
		}
		if t == kind || name == kind {
			return name
		}
	}
	return ""
}

func (c ResolveCtx) domain() string { return "localhost" }
