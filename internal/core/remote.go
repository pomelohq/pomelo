package core

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/pomelohq/pomelo/internal/agent/claude"
	"github.com/pomelohq/pomelo/internal/appstate"
	"github.com/pomelohq/pomelo/internal/paths"
	"github.com/pomelohq/pomelo/internal/remote"
)

// defaultRemotePort is a stable LAN port (avoiding dev-proxy 8767 / webhook 8766)
// so a paired phone reconnects across app restarts without re-pairing. The dev
// build uses a different port so it never collides with a released app running
// side by side.
func defaultRemotePort() int {
	exe, _ := os.Executable()
	if strings.Contains(filepath.Base(exe), "Dev") {
		return 8769
	}
	return 8768
}

// startRemote brings the LAN control listener up on launch when the user has
// enabled it. It is a genuinely-external listener (like dev-proxy/webhook), so
// starting it does not make the app itself HTTP-driven.
func (s *Server) startRemote() {
	if appstate.Load(s.session()).Remote.Enabled {
		s.remoteStart()
	}
}

func (s *Server) remoteStart() map[string]any {
	s.remoteMu.Lock()
	defer s.remoteMu.Unlock()
	if s.remoteSrv != nil {
		return s.remoteInfoLocked()
	}

	st := appstate.Load(s.session())
	if st.Remote.Token == "" {
		st.Remote.Token = newRemoteToken()
	}
	st.Remote.Enabled = true
	_ = appstate.Save(s.session(), st)

	srv := remote.New(s, st.Remote.Token, Version)
	srv.SetStreamer(s)
	srv.SetPTYStreamer(s)
	if cert, fp, err := remote.LoadOrCreateCert(paths.StatePath("remote")); err == nil {
		srv.SetTLS(cert, fp)
	} else {
		return map[string]any{"ok": false, "error": "remote cert: " + err.Error()}
	}
	// A stable port keeps a paired phone working across restarts; fall back to an
	// ephemeral one only if it is taken (e.g. a lingering instance).
	port := st.Remote.Port
	if port == 0 {
		port = defaultRemotePort()
	}
	if _, err := srv.Start(fmt.Sprintf("0.0.0.0:%d", port)); err != nil {
		if _, err2 := srv.Start("0.0.0.0:0"); err2 != nil {
			return map[string]any{"ok": false, "error": err2.Error()}
		}
	}
	s.remoteSrv = srv
	// Warm the PR cache in the background so the phone's first load is instant
	// instead of waiting on GitHub for every workspace.
	go func() {
		if s.pr != nil {
			_ = s.pr.AllPRs()
		}
	}()
	return s.remoteInfoLocked()
}

func (s *Server) remoteStop() map[string]any {
	s.remoteMu.Lock()
	defer s.remoteMu.Unlock()
	if s.remoteSrv != nil {
		s.remoteSrv.Stop()
		s.remoteSrv = nil
	}
	st := appstate.Load(s.session())
	st.Remote.Enabled = false
	_ = appstate.Save(s.session(), st)
	return map[string]any{"ok": true, "enabled": false}
}

// RemoteSet toggles the LAN listener and returns pairing info when enabled.
func (s *Server) RemoteSet(enabled bool) map[string]any {
	if enabled {
		return s.remoteStart()
	}
	return s.remoteStop()
}

func (s *Server) RemoteInfo() map[string]any {
	s.remoteMu.Lock()
	defer s.remoteMu.Unlock()
	return s.remoteInfoLocked()
}

func (s *Server) remoteInfoLocked() map[string]any {
	st := appstate.Load(s.session())
	info := map[string]any{
		"enabled": s.remoteSrv != nil,
		"host":    lanIP(),
	}
	if s.remoteSrv != nil {
		_, port, _ := net.SplitHostPort(s.remoteSrv.Addr())
		info["port"] = port
		info["token"] = st.Remote.Token
		info["fingerprint"] = s.remoteSrv.Fingerprint()
		info["hosts"] = hostIPs() // LAN + Tailscale, so the phone can pick its route
	}
	return info
}

func newRemoteToken() string {
	b := make([]byte, 24)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}

// lanIP returns the machine's primary private IPv4, the address a phone on the
// same network (or VPN) dials. Empty when only loopback is up.
func lanIP() string {
	if hs := hostIPs(); len(hs) > 0 {
		return hs[0]
	}
	return ""
}

// hostIPs lists the reachable IPv4s a phone can dial, LAN first then Tailscale
// (CGNAT 100.64.0.0/10). Cert-pin is host-independent, so any of these works
// with the same token+fingerprint — the phone just needs the right one for its
// network (LAN at home, Tailscale IP when remote).
func hostIPs() []string {
	addrs, err := net.InterfaceAddrs()
	if err != nil {
		return nil
	}
	var lan, ts []string
	for _, a := range addrs {
		ipnet, ok := a.(*net.IPNet)
		if !ok || ipnet.IP.IsLoopback() {
			continue
		}
		ip := ipnet.IP.To4()
		if ip == nil || ip[3] == 0 { // skip network addresses (docker bridges show x.x.x.0)
			continue
		}
		if ip[0] == 100 && ip[1] >= 64 && ip[1] <= 127 { // 100.64.0.0/10 (Tailscale/CGNAT)
			ts = append(ts, ip.String())
		} else if ip.IsPrivate() {
			lan = append(lan, ip.String())
		}
	}
	return append(lan, ts...)
}

// SetWsOrder records the desktop's drag-order of workspace ids so the remote
// (phone) can mirror the same order. Pushed from the app, not persisted.
func (s *Server) SetWsOrder(order []string) {
	s.wsOrderMu.Lock()
	s.wsOrder = append([]string(nil), order...)
	s.wsOrderMu.Unlock()
}

func (s *Server) WsOrder() []string {
	s.wsOrderMu.Lock()
	defer s.wsOrderMu.Unlock()
	return append([]string(nil), s.wsOrder...)
}

// CachedClaudeUsage serves the Claude usage snapshot with a 60s TTL so multiple
// remote clients polling don't hammer the upstream usage endpoint (429). If a
// fresh fetch fails but we hold a recent good value, keep serving that.
func (s *Server) CachedClaudeUsage() claude.Usage {
	// The desktop pushes a fresh value every ~60s (SetClaudeUsageJSON); serve that
	// within a 2-min window so the remote never calls the rate-limited upstream.
	s.usageMu.Lock()
	if s.usageVal.OK && time.Since(s.usageAt) < 120*time.Second {
		v := s.usageVal
		s.usageMu.Unlock()
		return v
	}
	// Back off: at most one upstream attempt per 60s even on failure, so a busy
	// client (5s poll) can't hold the account in a 429 loop.
	if !s.usageAttempt.IsZero() && time.Since(s.usageAttempt) < 60*time.Second {
		v := s.usageVal
		s.usageMu.Unlock()
		return v
	}
	s.usageAttempt = time.Now()
	s.usageMu.Unlock()

	u := claude.FetchUsage()
	s.usageMu.Lock()
	defer s.usageMu.Unlock()
	if u.OK {
		s.usageVal, s.usageAt = u, time.Now()
		return u
	}
	if s.usageVal.OK && time.Since(s.usageAt) < 10*time.Minute {
		return s.usageVal
	}
	return u
}

// SetClaudeUsageJSON stores usage the desktop already fetched (for its own chip),
// so the remote serves that instead of calling the rate-limited upstream itself.
func (s *Server) SetClaudeUsageJSON(raw []byte) {
	var u claude.Usage
	if err := json.Unmarshal(raw, &u); err != nil || !u.OK {
		return
	}
	s.usageMu.Lock()
	s.usageVal, s.usageAt = u, time.Now()
	s.usageMu.Unlock()
}
