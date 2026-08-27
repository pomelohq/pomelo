package services

import (
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
)

func TestResolveTokensV2(t *testing.T) {
	cfg := &config.Config{
		Session: "acme",
		Repos: map[string]*config.Dir{
			"acme-api": {Alias: "api", ServiceOrder: []string{"server"},
				Services: map[string]*config.Service{"server": {}}},
		},
		SharedServices: map[string]*config.SharedServiceDef{
			"postgres": {Type: "postgres", DBUser: "acme", DBPassword: "pw"},
		},
	}
	ctx := ResolveCtx{Cfg: cfg, Branch: "proj-1147-add-log", WsKey: PortWsKey("proj-1147-add-log"),
		DBNames: map[string]string{"main": "acme_proj_1147_add_log"}}

	cases := []struct{ in, want string }{
		{"{{branch}}", "proj-1147-add-log"},
		{"{{branch.safe}}", "proj-1147-add-log"},
		{"{{api.server.path}}", "/_pom_dev/api/server"},
		{"{{api.server.host}}", "server.api.proj-1147.localhost"},
		{"{{api.server.url}}", "http://server.api.proj-1147.localhost:8767"},
		{"{{api.server.ws}}", "ws://server.api.proj-1147.localhost:8767"},
		{"{{shared.postgres.user}}", "acme"},
		{"{{shared.postgres.pass}}", "pw"},
		{"{{shared.postgres.host}}", "127.0.0.1"},
		{"{{db.main}}", "acme_proj_1147_add_log"},
		{"DB={{db.main}}?x=1", "DB=acme_proj_1147_add_log?x=1"},
		{"{{nope.x.y}}", "{{nope.x.y}}"},
	}
	for _, c := range cases {
		if got := ResolveTokens(c.in, ctx); got != c.want {
			t.Errorf("ResolveTokens(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}
