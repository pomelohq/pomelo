package core

import (
	"fmt"
	"net"
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
)

func proxyTestCfg() *config.Config {
	return &config.Config{
		Repos: map[string]*config.Dir{
			"myrepo": {
				Alias:        "web",
				ServiceOrder: []string{"portal", "crm"},
				Services: map[string]*config.Service{
					"portal": {Cmd: "run portal"},
					"crm":    {Cmd: "run crm"},
				},
			},
			"apirepo": {
				Alias:        "api",
				ServiceOrder: []string{"server"},
				Services:     map[string]*config.Service{"server": {Cmd: "run api"}},
			},
		},
	}
}

func TestDevProxyURLFor(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	port := ln.Addr().(*net.TCPAddr).Port
	s := &Server{Addr: fmt.Sprintf(":%d", port-2)}
	cfg := proxyTestCfg()

	cases := []struct{ alias, svc, want string }{
		{"web", "portal", fmt.Sprintf("http://portal.web.feat-login.localhost:%d/", port)},
		{"web", "crm", fmt.Sprintf("http://crm.web.feat-login.localhost:%d/", port)},
		{"api", "server", fmt.Sprintf("http://server.api.feat-login.localhost:%d/", port)},
	}
	for _, c := range cases {
		if got := s.devProxyURLFor(cfg, "feat-login", c.alias, c.svc); got != c.want {
			t.Errorf("%s/%s: got %q want %q", c.alias, c.svc, got, c.want)
		}
	}
}

func TestDevProxyURLForNotListening(t *testing.T) {
	ln, _ := net.Listen("tcp", "127.0.0.1:0")
	port := ln.Addr().(*net.TCPAddr).Port
	ln.Close()
	s := &Server{Addr: fmt.Sprintf(":%d", port-2)}
	if got := s.devProxyURLFor(proxyTestCfg(), "feat-login", "web", "portal"); got != "" {
		t.Errorf("proxy not listening must return empty, got %q", got)
	}
}

func TestPickProxyPort(t *testing.T) {
	// Allocated port wins and the live scan is never consulted — a service is told to
	// bind exactly its allocated port, so a transient build socket must not override it.
	scanned := false
	if got := pickProxyPort(59851, func() int { scanned = true; return 3000 }); got != 59851 {
		t.Errorf("allocated port: got %d want 59851", got)
	}
	if scanned {
		t.Error("live scan ran while an allocated port was available")
	}
	// No lease → fall back to the live scan.
	if got := pickProxyPort(0, func() int { return 4000 }); got != 4000 {
		t.Errorf("no lease: got %d want 4000 (live scan)", got)
	}
}
