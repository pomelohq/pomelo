package services

import (
	"fmt"
	"os"
	"strings"
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
)

func TestSharedComposePortMatchesResolve(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	dir := t.TempDir()
	session := t.Name()
	shared := map[string]*config.SharedServiceDef{
		"redis": {Image: "redis:7-alpine", Ports: []string{"6379"}},
	}
	InitNetwork(dir, session, &config.Config{Session: session, SharedServices: shared})

	path := GenerateSharedCompose(dir, session, shared)
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read compose: %v", err)
	}
	// Compose must publish redis on the same port the template resolver hands out;
	// otherwise the container listens where generated URLs never point.
	want := fmt.Sprintf("\"%d:6379\"", SharedPort("redis"))
	if !strings.Contains(string(data), want) {
		t.Fatalf("compose must publish redis on the resolved port %s\n%s", want, data)
	}
}

func TestResolveSharedInstanceAware(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	projectDir := t.TempDir()
	cfg := &config.Config{
		Session:        t.Name(),
		SharedServices: map[string]*config.SharedServiceDef{"redis": {Ports: []string{"6379"}}},
	}
	InitNetwork(projectDir, cfg.Session, cfg)
	base := SharedPort("redis")

	// capacity 1 slot/instance -> two workspaces spread onto instances 0 and 1.
	AllocateSlot("redis", "ws-a", 1, uint16(base))
	AllocateSlot("redis", "ws-b", 1, uint16(base))

	sawInst1 := false
	for _, ws := range []string{"ws-a", "ws-b"} {
		c := ResolveCtx{Cfg: cfg, WsKey: ws}
		inst := c.sharedInstance("redis")
		if inst == 1 {
			sawInst1 = true
		}
		got, _ := c.resolveShared("redis", "port")
		if want := fmt.Sprintf("%d", base+inst); got != want {
			t.Errorf("%s: resolveShared port = %s, want base+instance = %s", ws, got, want)
		}
	}
	if !sawInst1 {
		t.Fatal("a workspace must resolve to instance 1's port, not always instance 0")
	}
}

func TestBranchSafe(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"main", "main"},
		{"feature/login", "feature_login"},
		{"task-524", "task-524"},
		{"feature/task-524-fix", "feature_task-524-fix"},
		{"release/v1.0.0", "release_v1.0.0"},
	}
	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			if got := BranchSafe(tt.input); got != tt.want {
				t.Errorf("BranchSafe(%q) = %q, want %q", tt.input, got, tt.want)
			}
		})
	}
}

func TestExtractPortFromCmd(t *testing.T) {
	tests := []struct {
		cmd  string
		want uint16
	}{
		{"go run . --port 3000", 3000},
		{"npm run dev --port 8080", 8080},
		{"rails server -p 4000", 4000},
		{"./server --port=9090", 9090},
		{"go run .", 0},
		{"", 0},
	}
	for _, tt := range tests {
		t.Run(tt.cmd, func(t *testing.T) {
			if got := ExtractPortFromCmd(tt.cmd); got != tt.want {
				t.Errorf("ExtractPortFromCmd(%q) = %d, want %d", tt.cmd, got, tt.want)
			}
		})
	}
}

func TestWorkspaceFolderPath(t *testing.T) {
	got := WorkspaceFolderPath("/home/user/project", "task-524")
	want := "/home/user/project/workspace--task-524"
	if got != want {
		t.Errorf("WorkspaceFolderPath() = %q, want %q", got, want)
	}
}

func TestValidateBranchName(t *testing.T) {
	tests := []struct {
		branch  string
		wantErr bool
	}{
		{"main", false},
		{"feature/login", false},
		{"task-524", false},
		{"", true},
		{"..", true},
		{"../etc/passwd", true},
		{"/absolute", true},
		{"~/home", true},
	}
	for _, tt := range tests {
		t.Run(tt.branch, func(t *testing.T) {
			err := ValidateBranchName(tt.branch)
			if (err != nil) != tt.wantErr {
				t.Errorf("ValidateBranchName(%q) error = %v, wantErr %v", tt.branch, err, tt.wantErr)
			}
		})
	}
}

func TestFileExistsAndDirExists(t *testing.T) {
	dir := t.TempDir()
	if !DirExists(dir) {
		t.Error("DirExists should return true for temp dir")
	}
	if FileExists(dir) {
		t.Error("FileExists should return false for directory")
	}
	if FileExists(dir + "/nonexistent") {
		t.Error("FileExists should return false for nonexistent")
	}
}

func TestContainsStr(t *testing.T) {
	ss := []string{"a", "b", "c"}
	if !ContainsStr(ss, "b") {
		t.Error("expected true for 'b'")
	}
	if ContainsStr(ss, "d") {
		t.Error("expected false for 'd'")
	}
	if ContainsStr(nil, "a") {
		t.Error("expected false for nil slice")
	}
}

func TestSharedPort(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	projectDir := t.TempDir()
	cfg := &config.Config{
		Session: t.Name(),
		SharedServices: map[string]*config.SharedServiceDef{
			"postgres": {Ports: []string{"5432"}},
			"redis":    {Ports: []string{"6379"}},
			"minio":    {Ports: []string{"9000", "9090"}},
		},
	}
	InitNetwork(projectDir, cfg.Session, cfg)

	type pb struct {
		port, base int
	}
	all := map[string]pb{
		"postgres": {SharedPort("postgres"), 5432},
		"redis":    {SharedPort("redis"), 6379},
		"minio:0":  {SharedPortAt("minio", 0), 9000},
		"minio:1":  {SharedPortAt("minio", 1), 9090},
	}
	seen := map[int]bool{}
	for name, v := range all {
		p := v.port
		if p < v.base || p > v.base+100 {
			t.Errorf("%s port %d not near its real port %d (+100)", name, p, v.base)
		}
		if seen[p] {
			t.Errorf("shared port collision at %d (%s)", p, name)
		}
		seen[p] = true
	}
}

func TestContainerPort(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"19305:5432", "5432"},
		{"5432", "5432"},
		{"9000", "9000"},
		{"19309:9000", "9000"},
	}
	for _, tt := range tests {
		if got := ContainerPort(tt.input); got != tt.want {
			t.Errorf("ContainerPort(%q) = %q, want %q", tt.input, got, tt.want)
		}
	}
}
