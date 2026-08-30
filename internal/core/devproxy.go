package core

import (
	"context"
	"fmt"
	"log"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/services"
)

type proxyCtxKey int

const (
	proxyTargetKey proxyCtxKey = iota
	proxyPrefixKey
	proxyExtURLKey
)

var (
	reCookiePath       = regexp.MustCompile(`(?i)(;\s*path=)(/[^;]*)`)
	reCookieSecure     = regexp.MustCompile(`(?i);\s*secure\b`)
	reCookieDomain     = regexp.MustCompile(`(?i);\s*domain=[^;]*`)
	reCookieSameSiteNo = regexp.MustCompile(`(?i);\s*samesite=none`)
	reCookieHasPath    = regexp.MustCompile(`(?i);\s*path=`)
)

func rewriteSetCookie(resp *http.Response, prefix string) {
	cookies := resp.Header["Set-Cookie"]
	if len(cookies) == 0 {
		return
	}
	for i, c := range cookies {
		if prefix != "" && prefix != "/" {
			c = reCookiePath.ReplaceAllStringFunc(c, func(m string) string {
				g := reCookiePath.FindStringSubmatch(m)
				return g[1] + prefix + g[2]
			})
		}
		c = reCookieSecure.ReplaceAllString(c, "")
		cookies[i] = c
	}
	resp.Header["Set-Cookie"] = cookies
}

func (s *Server) devProxy() *httputil.ReverseProxy {
	s.dpOnce.Do(func() {
		transport := &http.Transport{
			MaxIdleConns:        128,
			MaxIdleConnsPerHost: 24,
			IdleConnTimeout:     30 * time.Second,
			DialContext:         (&net.Dialer{Timeout: 5 * time.Second, KeepAlive: 30 * time.Second}).DialContext,
		}
		s.dp = &httputil.ReverseProxy{
			Transport: transport,
			Director: func(req *http.Request) {
				host, _ := req.Context().Value(proxyTargetKey).(string)
				req.URL.Scheme = "http"
				req.URL.Host = host
			},
			ModifyResponse: func(resp *http.Response) error {
				prefix, _ := resp.Request.Context().Value(proxyPrefixKey).(string)
				rewriteSetCookie(resp, prefix)
				return nil
			},
			ErrorHandler: func(w http.ResponseWriter, _ *http.Request, err error) {
				http.Error(w, "dev-proxy: backend not reachable — is the service running? ("+err.Error()+")", http.StatusBadGateway)
			},
		}
	})
	return s.dp
}

type proxyPortEntry struct {
	port int
	at   time.Time
}

// resolveProxyPort caches port resolution briefly: a dev server (e.g. Vite)
// fires hundreds of module requests per page load, and resolving live can shell
// out to list a process's listening sockets — doing that per request pegs the CPU.
func (s *Server) resolveProxyPort(branchLabel, target string) int {
	key := branchLabel + "\x00" + target
	now := time.Now()
	s.ppMu.Lock()
	if s.ppCache == nil {
		s.ppCache = map[string]proxyPortEntry{}
	}
	if e, ok := s.ppCache[key]; ok && e.port != 0 && now.Sub(e.at) < 3*time.Second {
		s.ppMu.Unlock()
		return e.port
	}
	s.ppMu.Unlock()

	port := s.resolveProxyPortUncached(branchLabel, target)
	if port != 0 {
		s.ppMu.Lock()
		s.ppCache[key] = proxyPortEntry{port: port, at: now}
		s.ppMu.Unlock()
	}
	return port
}

func (s *Server) resolveProxyPortUncached(branchLabel, target string) int {
	branch := s.branchForHostLabel(branchLabel)
	if branch == "" {
		return 0
	}
	svcKey, ok := resolveSvcKey(s.cfg(), target)
	if !ok {
		return 0
	}
	allocated := services.Port(s.WorkspaceRoot, services.PortWsKey(branch), svcKey)
	return pickProxyPort(allocated, func() int { return s.liveServicePort(branch, svcKey) })
}

// pickProxyPort prefers the allocated port: the service is told to bind exactly it, so
// live-scanning the process tree while it builds can latch onto a transient bundler/HMR
// socket and route there. Only scan when there is no lease at all.
func pickProxyPort(allocated int, liveScan func() int) int {
	if allocated > 0 {
		return allocated
	}
	return liveScan()
}

func (s *Server) liveServicePort(branch, svcKey string) int {
	i := strings.IndexByte(svcKey, '~')
	if i < 0 {
		return 0
	}
	alias, svc := svcKey[:i], svcKey[i+1:]
	cfg := s.cfg()
	repo := ""
	for name, d := range cfg.Repos {
		a := d.Alias
		if a == "" {
			a = name
		}
		if a == alias || name == alias {
			repo = name
			break
		}
	}
	if repo == "" {
		return 0
	}
	holder := services.ServiceHolderName(cfg.Session, branch, repo, svc)
	return ptyhost.ListeningPortInTree(holder, 10000, 65535)
}

func stripProxyPrefix(r *http.Request, prefix string) {
	p := strings.TrimPrefix(r.URL.Path, prefix)
	if p == "" || p[0] != '/' {
		p = "/" + p
	}
	r.URL.Path = p
	r.URL.RawPath = ""
}

func (s *Server) devProxyPort() int {
	if s.cfg() == nil {
		return 0
	}
	return s.webPort() + 2
}

func (s *Server) devProxyListening() bool {
	port := s.devProxyPort()
	return port != 0 && controlPortListening(port)
}

func (s *Server) startDevProxy() {
	port := s.devProxyPort()
	if port == 0 {
		return
	}
	handler := http.HandlerFunc(s.handleDevProxy)
	bound := false
	for _, host := range []string{"127.0.0.1", "[::1]"} {
		ln, err := net.Listen("tcp", fmt.Sprintf("%s:%d", host, port))
		if err != nil {
			continue
		}
		bound = true
		go func() { _ = http.Serve(ln, handler) }()
	}
	if !bound {
		log.Printf("dev proxy: port %d already in use — skipping (another pom owns it); service URLs will not route", port)
	}
}

func (s *Server) handleDevProxy(w http.ResponseWriter, r *http.Request) {
	cfg := s.cfg()
	if cfg == nil {
		http.Error(w, "dev proxy not configured", http.StatusServiceUnavailable)
		return
	}
	const domain = "localhost"
	host := r.Host
	if i := strings.IndexByte(host, ':'); i >= 0 {
		host = host[:i]
	}
	rest := strings.TrimSuffix(host, "."+domain)
	if rest == host {
		http.Error(w, "open a workspace at http://<service>.<repo>.<branch>."+domain, http.StatusNotFound)
		return
	}
	labels := strings.Split(rest, ".")

	if len(labels) == 2 && labels[1] == cfg.Session && sharedRelayed(cfg, labels[0]) {
		s.proxyToShared(w, r, labels[0])
		return
	}

	branchLabel := labels[len(labels)-1]

	if bePath := strings.TrimPrefix(r.URL.Path, "/_pom_dev/"); bePath != r.URL.Path {
		parts := strings.SplitN(bePath, "/", 3)
		if len(parts) >= 2 && parts[0] != "" && parts[1] != "" {
			prefix := "/_pom_dev/" + parts[0] + "/" + parts[1]
			origPath := r.URL.Path
			r = r.WithContext(context.WithValue(r.Context(), proxyPrefixKey, prefix))
			stripProxyPrefix(r, prefix)
			rec := &statusRecorder{ResponseWriter: w}
			start := time.Now()
			entry := ProxyLogEntry{Time: nowStamp(), Method: r.Method, Path: origPath, Repo: parts[0], Svc: parts[1]}
			if prof, ov := s.activeEnvOverrideProfile(labels, parts[0], parts[1]); ov != "" {
				entry.Profile, entry.Target = prof, ov
				s.proxyToURL(rec, r, ov)
			} else {
				entry.Profile = "local"
				entry.Target = s.proxyLocalTarget(branchLabel, parts[0]+"/"+parts[1])
				s.proxyToWorkspaceService(rec, r, branchLabel, parts[0]+"/"+parts[1])
			}
			entry.Status, entry.Ms = rec.status, time.Since(start).Milliseconds()
			s.proxyLog.add(entry)
			return
		}
	}

	if len(labels) == 3 {
		s.proxyToWorkspaceService(w, r, branchLabel, labels[1]+"/"+labels[0])
		return
	}
	http.Error(w, "no dev-proxy route for "+host+r.URL.Path, http.StatusNotFound)
}

// Ticket label only when it routes uniquely; else the full host (two workspaces
// sharing a ticket prefix collide, so only the full branch host resolves).
func (s *Server) proxyHostLabel(branch string) string {
	if wl := services.WorkspaceLabel(branch); s.branchForHostLabel(wl) == branch {
		return wl
	}
	return services.BranchHost(branch)
}

func (s *Server) devProxyURLFor(cfg *config.Config, branch, alias, svcName string) string {
	if cfg == nil {
		return ""
	}
	port := s.webPort() + 2
	if !controlPortListening(port) {
		return ""
	}
	const domain = "localhost"
	return fmt.Sprintf("http://%s.%s.%s.%s:%d/", svcName, alias, s.proxyHostLabel(branch), domain, port)
}

func controlPortListening(port int) bool {
	c, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 300*time.Millisecond)
	if err != nil {
		return false
	}
	_ = c.Close()
	return true
}

func (s *Server) devProxySharedURL(cfg *config.Config, name string) string {
	if !sharedRelayed(cfg, name) || !s.devProxyListening() {
		return ""
	}
	return fmt.Sprintf("http://%s.%s.localhost:%d/", name, cfg.Session, s.devProxyPort())
}

func sharedRelayed(cfg *config.Config, name string) bool {
	return cfg != nil && cfg.SharedServices[name] != nil
}

func (s *Server) proxyToShared(w http.ResponseWriter, r *http.Request, name string) {
	port := services.SharedPortAt(name, 0)
	if port == 0 {
		http.Error(w, "shared service "+name+" is not running", http.StatusBadGateway)
		return
	}
	host := services.BindIP() + ":" + strconv.Itoa(port)
	ctx := context.WithValue(r.Context(), proxyTargetKey, host)
	s.devProxy().ServeHTTP(w, r.WithContext(ctx))
}

func (s *Server) activeEnvOverrideProfile(hostLabels []string, targetRepo, targetSvc string) (string, string) {
	cfg := s.cfg()
	if cfg == nil || len(cfg.Environments) == 0 || len(hostLabels) == 0 {
		return "", ""
	}
	branchLabel := hostLabels[len(hostLabels)-1]
	branch := s.branchForHostLabel(branchLabel)
	if branch == "" {
		return "", ""
	}
	st := services.LoadWorkspaceState(services.WorkspaceRootDir(s.WorkspaceRoot, branch, branch == cfg.GlobalDefaultBranch()))
	profKeyRepo, profKeySvc := targetRepo, targetSvc
	if len(hostLabels) == 3 {
		profKeySvc, profKeyRepo = hostLabels[0], hostLabels[1]
	}
	prof := ""
	if st.ServiceEnvs != nil {
		if p, ok := st.ServiceEnvs[profKeyRepo+"/"+profKeySvc]; ok {
			prof = p
		} else if p, ok := st.ServiceEnvs[profKeyRepo]; ok {
			prof = p
		}
	}
	if prof == "" || prof == "local" {
		return "", ""
	}
	if m, ok := cfg.Environments[prof]; ok {
		return prof, m[targetRepo+"."+targetSvc]
	}
	return prof, ""
}

func (s *Server) proxyLocalTarget(branchLabel, target string) string {
	if port := s.resolveProxyPort(branchLabel, target); port > 0 {
		return services.BindIP() + ":" + strconv.Itoa(port)
	}
	return "(no local port)"
}

func rewriteSetCookieExternal(resp *http.Response, prefix string) {
	cookies := resp.Header["Set-Cookie"]
	if len(cookies) == 0 {
		return
	}
	for i, c := range cookies {
		if prefix != "" && prefix != "/" {
			if reCookieHasPath.MatchString(c) {
				c = reCookiePath.ReplaceAllStringFunc(c, func(m string) string {
					g := reCookiePath.FindStringSubmatch(m)
					return g[1] + prefix + g[2]
				})
			} else {
				c += "; Path=" + prefix + "/"
			}
		}
		c = reCookieDomain.ReplaceAllString(c, "")
		c = reCookieSecure.ReplaceAllString(c, "")
		c = reCookieSameSiteNo.ReplaceAllString(c, "; SameSite=Lax")
		cookies[i] = c
	}
	resp.Header["Set-Cookie"] = cookies
}

func (s *Server) extProxy() *httputil.ReverseProxy {
	s.epOnce.Do(func() {
		transport := &http.Transport{
			MaxIdleConns:        64,
			MaxIdleConnsPerHost: 12,
			IdleConnTimeout:     30 * time.Second,
			DialContext:         (&net.Dialer{Timeout: 8 * time.Second, KeepAlive: 30 * time.Second}).DialContext,
		}
		s.ep = &httputil.ReverseProxy{
			Transport: transport,
			Director: func(req *http.Request) {
				u, _ := req.Context().Value(proxyExtURLKey).(*url.URL)
				if u == nil {
					return
				}
				req.URL.Scheme, req.URL.Host, req.Host = u.Scheme, u.Host, u.Host
			},
			ModifyResponse: func(resp *http.Response) error {
				prefix, _ := resp.Request.Context().Value(proxyPrefixKey).(string)
				rewriteSetCookieExternal(resp, prefix)
				return nil
			},
			ErrorHandler: func(w http.ResponseWriter, _ *http.Request, err error) {
				http.Error(w, "dev-proxy: remote env not reachable ("+err.Error()+")", http.StatusBadGateway)
			},
		}
	})
	return s.ep
}

func (s *Server) proxyToURL(w http.ResponseWriter, r *http.Request, raw string) {
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" {
		http.Error(w, "dev-proxy: bad env override URL: "+raw, http.StatusBadGateway)
		return
	}
	ctx := context.WithValue(r.Context(), proxyExtURLKey, u)
	s.extProxy().ServeHTTP(w, r.WithContext(ctx))
}

func (s *Server) proxyToWorkspaceService(w http.ResponseWriter, r *http.Request, branchLabel, target string) {
	port := s.resolveProxyPort(branchLabel, target)
	if port == 0 {
		http.Error(w, "no dev-proxy route for "+branchLabel+" / "+target, http.StatusBadGateway)
		return
	}
	host := services.BindIP() + ":" + strconv.Itoa(port)
	ctx := context.WithValue(r.Context(), proxyTargetKey, host)
	s.devProxy().ServeHTTP(w, r.WithContext(ctx))
}
