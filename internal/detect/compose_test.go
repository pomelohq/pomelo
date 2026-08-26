package detect_test

import (
	"testing"

	"github.com/pomelohq/pomelo/internal/detect"
)

func svcByName(cs []detect.ComposeService, name string) (detect.ComposeService, bool) {
	for _, c := range cs {
		if c.Name == name {
			return c, true
		}
	}
	return detect.ComposeService{}, false
}

func TestComposeClassify(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"compose.yml": `
services:
  db:
    image: postgres:16-alpine
  cache:
    image: redis:7
  search:
    image: docker.elastic.co/elasticsearch/elasticsearch:8.13.0
  storage:
    image: minio/minio
  queue:
    image: rabbitmq:3-management
  mail:
    image: axllent/mailpit
  web:
    build: .
  proxy:
    image: nginx:alpine
  weird:
    image: acme/some-internal-thing:1.2
  api:
    image: myapp-api:latest
  zk:
    image: zookeeper:3.9
`,
	})
	cs := detect.ParseCompose(root)

	check := func(name string, kind detect.ServiceKind, typ string, strat detect.Strategy) {
		c, ok := svcByName(cs, name)
		if !ok {
			t.Fatalf("%s not found", name)
		}
		if c.Kind != kind || c.Type != typ || c.Strategy != strat {
			t.Fatalf("%s: got kind=%s type=%s strat=%s", name, c.Kind, c.Type, c.Strategy)
		}
	}
	check("db", detect.KindShared, "postgres", detect.DatabasePerBranch)
	check("cache", detect.KindShared, "redis", detect.DBIndexSlot)
	check("search", detect.KindShared, "elasticsearch", detect.NamespacePrefix)
	check("storage", detect.KindShared, "minio", detect.BucketPerBranch)
	check("queue", detect.KindShared, "rabbitmq", detect.VhostPerBranch)
	check("mail", detect.KindShared, "mail", detect.SharedStateless)
	check("web", detect.KindApp, "", "")
	check("proxy", detect.KindProxy, "", "")
	check("weird", detect.KindShared, "custom", "")
}

func TestComposeCloneMethod(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"compose.yml": `
services:
  pg:
    image: postgres:16
  my:
    image: mysql:8.4
  mo:
    image: mongo:7
  cache:
    image: redis:7
  files:
    image: minio/minio
`,
	})
	cs := detect.ParseCompose(root)
	want := map[string]string{"pg": "template", "my": "dump-restore", "mo": "dump-restore", "cache": "none", "files": "none"}
	for name, cl := range want {
		c, ok := svcByName(cs, name)
		if !ok || c.Clone != cl {
			t.Fatalf("%s clone: got %q want %q", name, c.Clone, cl)
		}
	}
}

func TestComposeExtends(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"compose.yml": `
services:
  base:
    image: postgres:16
  db:
    extends:
      service: base
`,
	})
	cs := detect.ParseCompose(root)
	db, ok := svcByName(cs, "db")
	if !ok || db.Kind != detect.KindShared || db.Type != "postgres" {
		t.Fatalf("db via extends should be postgres shared: %+v", cs)
	}
}

func TestComposeExtendsFile(t *testing.T) {
	root := writeRepo(t, map[string]string{
		"base.yml": `
services:
  pg:
    image: postgres:16
`,
		"compose.yml": `
services:
  db:
    extends:
      file: base.yml
      service: pg
`,
	})
	db, ok := svcByName(detect.ParseCompose(root), "db")
	if !ok || db.Type != "postgres" {
		t.Fatalf("db via file extends should be postgres: %+v", db)
	}
}

func TestComposeNone(t *testing.T) {
	if cs := detect.ParseCompose(writeRepo(t, map[string]string{"README.md": "hi"})); cs != nil {
		t.Fatalf("expected nil for no compose, got %+v", cs)
	}
}
