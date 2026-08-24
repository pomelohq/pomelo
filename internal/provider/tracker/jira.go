package tracker

import (
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/httpx"
	"github.com/pomelohq/pomelo/internal/jira"
	"github.com/pomelohq/pomelo/internal/plugin"
	"github.com/pomelohq/pomelo/internal/secrets"
)

type Jira struct {
	cfg func() *config.Config

	mu     sync.Mutex
	cache  map[string]jira.Issue
	cached map[string]time.Time
}

func NewJira(cfg func() *config.Config) *Jira {
	return &Jira{cfg: cfg, cache: map[string]jira.Issue{}, cached: map[string]time.Time{}}
}

func (*Jira) Name() string { return "jira" }

func (f *Jira) Routes(mux *http.ServeMux) {
	mux.HandleFunc("/api/jira/issues", f.handleIssues)
	mux.HandleFunc("/api/jira/issue", f.handleIssue)
	mux.HandleFunc("/api/jira/test", f.handleTest)
	mux.HandleFunc("/api/jira/boards", f.handleBoards)
	mux.HandleFunc("/api/jira/sprint", f.handleSprint)
}

func (f *Jira) handleBoards(w http.ResponseWriter, r *http.Request) { httpx.Write(w, f.Boards()) }

func (f *Jira) Boards() map[string]any {
	jc := jira.Resolve(f.cfg())
	if jc == nil {
		return map[string]any{"configured": false}
	}
	boards, err := jc.Boards()
	if err != nil {
		return map[string]any{"configured": true, "boards": []any{}, "error": err.Error()}
	}
	return map[string]any{"configured": true, "boards": boards}
}

func (f *Jira) handleSprint(w http.ResponseWriter, r *http.Request) {
	board, _ := strconv.Atoi(r.URL.Query().Get("board"))
	if board == 0 {
		httpx.Err(w, http.StatusBadRequest, "missing board")
		return
	}
	httpx.Write(w, f.Sprint(board))
}

func (f *Jira) Sprint(board int) map[string]any {
	jc := jira.Resolve(f.cfg())
	if jc == nil {
		return map[string]any{"configured": false}
	}
	me, _ := jc.MyAccountID()
	issues, err := jc.CurrentSprintIssues(board)
	if err != nil {
		return map[string]any{"configured": true, "issues": []any{}, "error": err.Error()}
	}
	type outIssue struct {
		Key      string `json:"key"`
		Summary  string `json:"summary"`
		Status   string `json:"status"`
		Assignee string `json:"assignee"`
		Avatar   string `json:"avatar"`
		Sprint   string `json:"sprint"`
		Mine     bool   `json:"mine"`
	}
	out := make([]outIssue, 0, len(issues))
	for _, i := range issues {
		out = append(out, outIssue{Key: i.Key, Summary: i.Summary, Status: i.Status,
			Assignee: i.Assignee, Avatar: i.Avatar, Sprint: i.Sprint, Mine: me != "" && i.AccountID == me})
	}
	return map[string]any{"configured": true, "issues": out}
}

func (f *Jira) handleIssue(w http.ResponseWriter, r *http.Request) {
	key := strings.ToUpper(strings.TrimSpace(r.URL.Query().Get("key")))
	if key == "" {
		httpx.Err(w, http.StatusBadRequest, "missing key")
		return
	}
	httpx.Write(w, f.Issue(key))
}

func (f *Jira) Issue(key string) map[string]any {
	jc := jira.Resolve(f.cfg())
	if jc == nil {
		return map[string]any{"configured": false}
	}
	d, err := jc.IssueWithDescription(key)
	if err != nil || d == nil {
		return map[string]any{"configured": true, "key": key, "error": "not found"}
	}
	return map[string]any{
		"configured": true, "key": d.Key, "summary": d.Summary,
		"status": d.Status, "url": d.URL, "description": d.Description,
		"comments": d.Comments, "web_links": d.WebLinks,
	}
}

const jiraTTL = 60 * time.Second

func (f *Jira) handleIssues(w http.ResponseWriter, r *http.Request) {
	req, err := httpx.Read[struct {
		Branches []string `json:"branches"`
	}](r)
	if err != nil {
		httpx.Err(w, http.StatusBadRequest, "bad json")
		return
	}
	httpx.Write(w, f.Issues(req.Branches))
}

func (f *Jira) Issues(branches []string) map[string]any {
	jc := jira.Resolve(f.cfg())
	if jc == nil {
		return map[string]any{"configured": false}
	}
	keys := map[string]bool{}
	for _, b := range branches {
		if k := jira.KeyForBranch(b); k != "" {
			keys[k] = true
		}
	}
	f.warm(jc, keys)

	out := map[string]jira.Issue{}
	f.mu.Lock()
	for k := range keys {
		if iss, ok := f.cache[k]; ok && iss.Key != "" {
			out[k] = iss
		}
	}
	f.mu.Unlock()
	return map[string]any{"configured": true, "site": jc.Site, "issues": out}
}

func (f *Jira) warm(jc *jira.Client, keys map[string]bool) {
	var stale []string
	f.mu.Lock()
	for k := range keys {
		if time.Since(f.cached[k]) >= jiraTTL {
			stale = append(stale, k)
		}
	}
	f.mu.Unlock()
	if len(stale) == 0 {
		return
	}

	issues, err := jc.SearchByKeys(stale)
	if err != nil {
		return
	}
	now := time.Now()
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, k := range stale {
		f.cached[k] = now
	}
	for _, iss := range issues {
		f.cache[iss.Key] = iss
	}
}

func (f *Jira) handleTest(w http.ResponseWriter, r *http.Request) {
	req, _ := httpx.Read[struct{ Site, Email, Token string }](r)
	httpx.Write(w, f.Test(req.Site, req.Email, req.Token))
}

func (f *Jira) Test(site, email, token string) map[string]any {
	if site == "" || email == "" {
		return map[string]any{"ok": false, "error": "set site URL and account email first"}
	}
	session := ""
	if f.cfg() != nil {
		session = f.cfg().Session
	}
	token = strings.TrimSpace(token)
	if token == "" {
		if v, ok := secrets.Get(session, jira.TokenSecret); ok {
			token = v
		}
	}
	if token == "" {
		token = os.Getenv(jira.DefaultTokenEnv)
	}
	if token == "" {
		return map[string]any{"ok": false, "error": "no token — enter your Jira API token and save"}
	}
	name, addr, err := jira.Myself(site, email, token)
	if err != nil {
		msg := err.Error()
		if strings.Contains(msg, "401") || strings.Contains(msg, "403") {
			msg = "auth rejected — check email + token"
		}
		return map[string]any{"ok": false, "error": msg}
	}
	return map[string]any{"ok": true, "user": name, "email": addr}
}

var (
	_ Provider            = (*Jira)(nil)
	_ plugin.Feature      = (*Jira)(nil)
	_ plugin.HTTPProvider = (*Jira)(nil)
)
