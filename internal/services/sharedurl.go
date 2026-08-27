package services

import (
	"fmt"
	"path/filepath"

	"github.com/pomelohq/pomelo/internal/config"
)

func SharedServiceURL(session, service string) (string, error) {
	reg := LoadRegistry()
	dir := reg.Projects[session]
	if dir == "" {
		return "", fmt.Errorf("unknown session %q (see `pom status`)", session)
	}
	cfgPath := filepath.Join(dir, config.ConfigFileName)
	cfg, err := config.Load(cfgPath)
	if err != nil {
		return "", fmt.Errorf("load config for %q: %w", session, err)
	}
	InitNetwork(dir, session, cfg)
	SetSharedStable(session)

	svc := cfg.SharedServices[service]
	if svc == nil {
		return "", fmt.Errorf("no shared service %q in session %q", service, session)
	}
	host := cfg.SharedHost(service)
	if host == "" {
		host = BindIP() // explicit IPv4 loopback; see resolveShared
	}
	port := SharedPort(service)
	if port == 0 {
		return "", fmt.Errorf("shared service %q has no allocated port (is it running?)", service)
	}

	typ := svc.Type
	if typ == "" {
		typ = service
	}
	switch {
	case svc.DBUser != "" || typ == "postgres" || typ == "postgresql":
		user, pw := svc.DBUser, svc.DBPassword
		if user == "" {
			user = "postgres"
		}
		if pw == "" {
			pw = "postgres"
		}
		return fmt.Sprintf("postgresql://%s:%s@%s:%d", user, pw, host, port), nil
	case typ == "mysql" || typ == "mariadb":
		user := svc.DBUser
		if user == "" {
			user = "root"
		}
		return fmt.Sprintf("mysql://%s:%s@%s:%d", user, svc.DBPassword, host, port), nil
	case typ == "redis":
		return fmt.Sprintf("redis://%s:%d", host, port), nil
	default:
		return fmt.Sprintf("http://%s:%d", host, port), nil
	}
}
