package services

import (
	"fmt"
	"sort"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"github.com/pomelohq/pomelo/internal/config"
)

var (
	currentProjectDir string
	currentSession    string
)

func InitNetwork(projectDir, session string, cfg *config.Config) {
	currentProjectDir = projectDir
	currentSession = session
	RegisterProject(session, projectDir)

	ReserveSharedPorts(cfg.SharedServices)
}

// ReserveSharedPorts assigns a stable, persisted port lease to every shared
// service port. Both the compose generator and the template resolver must call
// this before reading SharedPort, else one falls back to the StableSharedPort
// hash while the other uses the lease and the container ends up on a port the
// generated URLs never point at.
func ReserveSharedPorts(shared map[string]*config.SharedServiceDef) {
	m := mgr()
	names := make([]string, 0, len(shared))
	for name := range shared {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		svc := shared[name]
		n := len(svc.Ports)
		if n == 0 {
			n = 1
		}
		for i := 0; i < n; i++ {
			m.AcquirePreferred(sharedLocalKeyAt(name, i), parsePortBase(svc.Ports, i), 100)
		}
	}
}

func parsePortBase(ports []string, i int) int {
	if i >= len(ports) {
		return 0
	}
	s := ports[i]
	if c := strings.IndexAny(s, ":/"); c >= 0 {
		s = s[:c]
	}
	n, _ := strconv.Atoi(strings.TrimSpace(s))
	return n
}

func Port(projectDir, wsKey, svcKey string) int {
	return mgr().portOf(svcLocalKey(wsKey, svcKey))
}

func PortEverBound(wsKey, svcKey string) bool {
	key := svcLocalKey(wsKey, svcKey)
	for _, l := range mgr().Snapshot() {
		if l.Key == key {
			return l.seenUp || l.State == PortRunning
		}
	}
	return false
}

var sharedStableSession atomic.Value

func SetSharedStable(session string) { sharedStableSession.Store(session) }

func SharedPort(svcName string) int {
	if p := mgr().portOf(sharedLocalKey(svcName)); p != 0 {
		return p
	}
	if s, _ := sharedStableSession.Load().(string); s != "" {
		return StableSharedPort(s, svcName)
	}
	return 0
}

func StableSharedPort(session, svcName string) int {
	const base, span = 20000, 10000
	var h uint32 = 2166136261
	for _, c := range session + "/" + svcName {
		h = (h ^ uint32(c)) * 16777619
	}
	return base + int(h%span)
}
func SharedPortAt(svcName string, portIndex int) int {
	if p := mgr().portOf(sharedLocalKeyAt(svcName, portIndex)); p != 0 {
		return p
	}
	if s, _ := sharedStableSession.Load().(string); s != "" {
		if portIndex == 0 {
			return StableSharedPort(s, svcName)
		}
		return StableSharedPort(s, fmt.Sprintf("%s#%d", svcName, portIndex))
	}
	return 0
}

func ClaimBlock(projectDir, wsKey string) int { return 0 }

func RelocateToCleanBlock(projectDir, wsKey string) int {
	mgr().ReleaseWorkspace(wsKey)
	return 0
}

func ReleaseBlock(projectDir, wsKey string) { mgr().ReleaseWorkspace(wsKey) }

func IsPortFree(port int) bool { return portBindable(port) }

func ReleaseServicePort(wsKey, svcKey string) { mgr().Release(svcLocalKey(wsKey, svcKey)) }

func WhoListensPort(port int) string {
	out, err := RunTimeout(2*time.Second, "", "lsof", "-nP", fmt.Sprintf("-iTCP:%d", port), "-sTCP:LISTEN", "-Fpc")
	if err != nil || len(out) == 0 {
		return ""
	}
	var pid, cmd string
	for _, line := range strings.Split(string(out), "\n") {
		if len(line) < 2 {
			continue
		}
		switch line[0] {
		case 'p':
			pid = line[1:]
		case 'c':
			cmd = line[1:]
		}
	}
	switch {
	case cmd != "" && pid != "":
		return fmt.Sprintf("%s (pid %s)", cmd, pid)
	case cmd != "":
		return cmd
	default:
		return ""
	}
}

func ContainerPort(portMapping string) string {
	parts := strings.SplitN(portMapping, ":", 2)
	if len(parts) == 2 {
		return parts[1]
	}
	return parts[0]
}
