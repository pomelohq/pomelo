package services

import (
	"math/rand"
	"net"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/paths"
)

var (
	currentMgr atomic.Pointer[portManager]
	mgrMu      sync.Mutex
)

func mgr() *portManager {
	if m := currentMgr.Load(); m != nil && m.session == currentSession {
		return m
	}
	mgrMu.Lock()
	defer mgrMu.Unlock()
	if m := currentMgr.Load(); m != nil && m.session == currentSession {
		return m
	}
	m := newPortManager(currentSession)
	currentMgr.Store(m)
	return m
}

func PortMgr() *portManager { return mgr() }

func acquireWorkspacePorts(cfg *config.Config, wsKey string) {
	m := mgr()
	for _, dirName := range cfg.RepoOrder {
		dir := cfg.Repos[dirName]
		if dir == nil {
			continue
		}
		alias := dir.Alias
		if alias == "" {
			alias = dirName
		}
		for _, svcName := range dir.ServiceOrder {
			if svc := dir.Services[svcName]; svc != nil && !svc.HasPort() {
				continue
			}
			m.Acquire(svcLocalKey(wsKey, alias+"~"+svcName))
		}
	}
}

func svcLocalKey(wsKey, svcKey string) string { return wsKey + "\x1f" + svcKey }
func sharedLocalKey(name string) string       { return "shared\x1f" + name }
func sharedLocalKeyAt(name string, idx int) string {
	if idx <= 0 {
		return sharedLocalKey(name)
	}
	return sharedLocalKey(name) + "#" + strconv.Itoa(idx)
}

type PortState string

const (
	PortAssigned PortState = "assigned"
	PortStarting PortState = "starting"
	PortRunning  PortState = "running"
)

type PortLease struct {
	Key       string
	Port      int
	Session   string
	State     PortState
	Since     time.Time
	seenUp    bool
	downSince time.Time // first probe miss after being up; zero = currently reachable
}

const (
	portLo        = 10000
	portHi        = 65535
	assignGrace   = 45 * time.Second
	reapDownGrace = 20 * time.Second
	leaseMagic    = "pom-port"
	leaseVer      = "v1"
)

type portCmd interface{ portCmd() }

type funcCmd func()

type acqCmd struct {
	key   string
	reply chan int
}
type markCmd struct {
	key   string
	state PortState
}
type relCmd struct{ key string }
type relWsCmd struct{ prefix string }
type reapCmd struct{ done chan struct{} }
type snapCmd struct{ reply chan []PortLease }

func (acqCmd) portCmd()   {}
func (markCmd) portCmd()  {}
func (relCmd) portCmd()   {}
func (relWsCmd) portCmd() {}
func (reapCmd) portCmd()  {}
func (snapCmd) portCmd()  {}
func (funcCmd) portCmd()  {}

type portManager struct {
	session string
	cmds    chan portCmd
	leases  map[string]*PortLease
	snap    atomic.Pointer[map[string]int]
	rng     *rand.Rand

	isUp     func(port int) bool
	bindable func(port int) bool
	now      func() time.Time
}

func newPortManager(session string) *portManager {
	m := &portManager{
		session:  session,
		cmds:     make(chan portCmd, 64),
		leases:   map[string]*PortLease{},
		rng:      rand.New(rand.NewSource(time.Now().UnixNano())),
		isUp:     portDialable,
		bindable: portBindable,
		now:      time.Now,
	}
	empty := map[string]int{}
	m.snap.Store(&empty)
	m.hydrate()
	go m.loop()
	return m
}

func (m *portManager) hydrate() {
	for port, l := range scanLeaseFiles() {
		if l.Session != m.session {
			continue
		}
		lease := l
		lease.Port = port
		lease.seenUp = l.State == PortRunning
		m.leases[l.Key] = &lease
	}
	m.publish()
}

func (m *portManager) loop() {
	for c := range m.cmds {
		switch c := c.(type) {
		case acqCmd:
			c.reply <- m.acquire(c.key)
		case markCmd:
			if l := m.leases[c.key]; l != nil && l.State != c.state {
				l.State = c.state
				switch c.state {
				case PortRunning:
					l.seenUp = true
				case PortStarting:
					l.Since = m.now()
				}
				writeLeaseFile(l)
				m.publish()
			}
		case relCmd:
			m.release(c.key)
		case relWsCmd:
			for key, l := range m.leases {
				if strings.HasPrefix(key, c.prefix) {
					delete(m.leases, key)
					_ = os.Remove(leaseFilePath(l.Port))
				}
			}
			m.publish()
		case reapCmd:
			m.reap()
			close(c.done)
		case snapCmd:
			out := make([]PortLease, 0, len(m.leases))
			for _, l := range m.leases {
				out = append(out, *l)
			}
			c.reply <- out
		case funcCmd:
			c()
		}
	}
}

func (m *portManager) acquire(key string) int {
	if l := m.leases[key]; l != nil {
		return l.Port
	}
	l := &PortLease{Key: key, Session: m.session, State: PortAssigned, Since: m.now()}
	l.Port = m.claim(l)
	if l.Port == 0 {
		return 0
	}
	m.leases[key] = l
	m.publish()
	return l.Port
}

func (m *portManager) claim(l *PortLease) int {
	dir := portsDir()
	_ = os.MkdirAll(dir, 0o755)
	for tries := 0; tries < 2000; tries++ {
		p := portLo + m.rng.Intn(portHi-portLo+1)
		if !m.bindable(p) {
			continue
		}
		f, err := os.OpenFile(filepath.Join(dir, strconv.Itoa(p)), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
		if err != nil {
			continue
		}
		l.Port = p
		_, _ = f.WriteString(formatLease(l))
		_ = f.Close()
		return p
	}
	return 0
}

func (m *portManager) release(key string) {
	l := m.leases[key]
	if l == nil {
		return
	}
	delete(m.leases, key)
	_ = os.Remove(leaseFilePath(l.Port))
	m.publish()
}

func (m *portManager) reap() {
	var dead []string
	changed := false
	for key, l := range m.leases {
		switch {
		case m.isUp(l.Port):
			l.downSince = time.Time{}
			if l.State != PortRunning {
				l.State, l.seenUp, changed = PortRunning, true, true
				writeLeaseFile(l)
			}
		case l.seenUp || l.State == PortRunning:
			// A previously-up port that fails one probe is usually a blip (a dev
			// server restarting on HMR), not a dead service — reclaiming here would
			// drop the port we authoritatively allocated and force a costly rescan.
			// Only reap after it stays unreachable for a sustained window.
			if l.downSince.IsZero() {
				l.downSince = m.now()
			} else if m.now().Sub(l.downSince) > reapDownGrace {
				dead = append(dead, key)
			}
		case l.State == PortStarting && m.now().Sub(l.Since) > assignGrace:
			dead = append(dead, key)
		}
	}
	for _, key := range dead {
		if l := m.leases[key]; l != nil {
			delete(m.leases, key)
			_ = os.Remove(leaseFilePath(l.Port))
		}
	}
	if changed || len(dead) > 0 {
		m.publish()
	}
}

func (m *portManager) publish() {
	snap := make(map[string]int, len(m.leases))
	for key, l := range m.leases {
		snap[key] = l.Port
	}
	m.snap.Store(&snap)
}

func (m *portManager) portOf(key string) int { return (*m.snap.Load())[key] }

func (m *portManager) SvcPorts(svcKey string) []int {
	suffix := "\x1f" + svcKey
	var out []int
	for key, port := range *m.snap.Load() {
		if strings.HasSuffix(key, suffix) {
			out = append(out, port)
		}
	}
	return out
}

func (m *portManager) Acquire(key string) int {
	reply := make(chan int, 1)
	m.cmds <- acqCmd{key: key, reply: reply}
	return <-reply
}

func (m *portManager) AcquirePreferred(key string, base, span int) int {
	reply := make(chan int, 1)
	m.cmds <- funcCmd(func() {
		if l := m.leases[key]; l != nil {
			reply <- l.Port
			return
		}
		l := &PortLease{Key: key, Session: m.session, State: PortAssigned, Since: m.now()}
		l.Port = m.claimPreferred(l, base, span)
		if l.Port == 0 {
			reply <- 0
			return
		}
		m.leases[key] = l
		m.publish()
		reply <- l.Port
	})
	return <-reply
}

func (m *portManager) claimPreferred(l *PortLease, base, span int) int {
	dir := portsDir()
	_ = os.MkdirAll(dir, 0o755)
	for off := 0; base > 0 && off <= span; off++ {
		p := base + off
		if p > 65535 {
			break
		}
		if !m.bindable(p) {
			continue
		}
		f, err := os.OpenFile(filepath.Join(dir, strconv.Itoa(p)), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
		if err != nil {
			continue
		}
		l.Port = p
		_, _ = f.WriteString(formatLease(l))
		_ = f.Close()
		return p
	}
	return m.claim(l)
}
func (m *portManager) Mark(key string, state PortState) { m.cmds <- markCmd{key: key, state: state} }
func (m *portManager) Release(key string)               { m.cmds <- relCmd{key: key} }

func (m *portManager) ReleaseWorkspace(wsKey string) { m.cmds <- relWsCmd{prefix: wsKey + "\x1f"} }
func (m *portManager) Reap() {
	done := make(chan struct{})
	m.cmds <- reapCmd{done: done}
	<-done
}
func (m *portManager) Snapshot() []PortLease {
	reply := make(chan []PortLease, 1)
	m.cmds <- snapCmd{reply: reply}
	return <-reply
}

func AllPortLeases() []PortLease {
	m := scanLeaseFiles()
	out := make([]PortLease, 0, len(m))
	for port, l := range m {
		l.Port = port
		out = append(out, l)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Port < out[j].Port })
	return out
}

func SplitLeaseKey(key string) (ws, svc string) {
	if i := strings.IndexByte(key, '\x1f'); i >= 0 {
		return key[:i], key[i+1:]
	}
	return "", key
}

func portsDir() string              { return paths.StatePath("ports.d") }
func leaseFilePath(port int) string { return filepath.Join(portsDir(), strconv.Itoa(port)) }

func formatLease(l *PortLease) string {
	return strings.Join([]string{
		leaseMagic, leaseVer, l.Session, l.Key, string(l.State),
		strconv.FormatInt(l.Since.UnixMilli(), 10),
	}, "\t") + "\n"
}

func parseLease(line string) (PortLease, bool) {
	f := strings.Split(strings.TrimRight(line, "\n"), "\t")
	if len(f) != 6 || f[0] != leaseMagic || f[1] != leaseVer {
		return PortLease{}, false
	}
	ms, err := strconv.ParseInt(f[5], 10, 64)
	if err != nil {
		return PortLease{}, false
	}
	return PortLease{Session: f[2], Key: f[3], State: PortState(f[4]), Since: time.UnixMilli(ms)}, true
}

func writeLeaseFile(l *PortLease) {
	_ = os.WriteFile(leaseFilePath(l.Port), []byte(formatLease(l)), 0o644)
}

func scanLeaseFiles() map[int]PortLease {
	out := map[int]PortLease{}
	entries, err := os.ReadDir(portsDir())
	if err != nil {
		return out
	}
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		port, err := strconv.Atoi(e.Name())
		if err != nil {
			continue
		}
		data, err := os.ReadFile(filepath.Join(portsDir(), e.Name()))
		if err != nil {
			continue
		}
		if l, ok := parseLease(string(data)); ok {
			l.Port = port
			out[port] = l
		}
	}
	return out
}

func portDialable(port int) bool {
	c, err := net.DialTimeout("tcp", net.JoinHostPort(BindIP(), strconv.Itoa(port)), 300*time.Millisecond)
	if err != nil {
		return false
	}
	_ = c.Close()
	return true
}

func portBindable(port int) bool {
	ln, err := net.Listen("tcp", net.JoinHostPort(BindIP(), strconv.Itoa(port)))
	if err != nil {
		return false
	}
	_ = ln.Close()
	return true
}
