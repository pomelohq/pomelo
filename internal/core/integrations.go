package core

import (
	"net/http"
	"os"
	"strings"

	"github.com/pomelohq/pomelo/internal/appstate"
	"github.com/pomelohq/pomelo/internal/jira"
	"github.com/pomelohq/pomelo/internal/provider/forge"
	"github.com/pomelohq/pomelo/internal/secrets"
)

func (s *Server) integrationsRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/integrations/status", s.handleIntegrationsStatus)
	mux.HandleFunc("/api/integrations/jira", s.handleJiraConfigSet)
}

func (s *Server) handleIntegrationsStatus(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.IntegrationsStatus())
}

func (s *Server) IntegrationsStatus() map[string]any {
	cfg := s.cfg()
	site, email, tokenEnv := "", "", ""
	if j := appstate.Load(s.session()).Jira; j.Site != "" || j.Email != "" {
		site, email = j.Site, j.Email
	}
	if tokenEnv == "" {
		tokenEnv = jira.DefaultTokenEnv
	}
	tok, hasSecret := secrets.Get(s.session(), jira.TokenSecret)
	return map[string]any{
		"jira": map[string]any{
			"configured": jira.Resolve(cfg) != nil,
			"site":       site,
			"email":      email,
			"token_set":  (hasSecret && tok != "") || os.Getenv(tokenEnv) != "",
		},
		"github": githubStatus(),
	}
}

func (s *Server) JiraConfigSet(site, email, token string) error {
	st := appstate.Load(s.session())
	st.Jira = appstate.JiraConfig{Site: site, Email: email}
	if err := appstate.Save(s.session(), st); err != nil {
		return err
	}
	if t := strings.TrimSpace(token); t != "" {
		return secrets.Set(s.session(), jira.TokenSecret, t)
	}
	return nil
}

func (s *Server) handleJiraConfigSet(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	req, err := readJSON[struct {
		Site  string `json:"site"`
		Email string `json:"email"`
		Token string `json:"token"`
	}](r)
	if err != nil {
		httpErr(w, http.StatusBadRequest, "bad json")
		return
	}
	if err := s.JiraConfigSet(req.Site, req.Email, req.Token); err != nil {
		httpErr(w, http.StatusInternalServerError, "%s", err.Error())
		return
	}
	writeJSON(w, map[string]any{"ok": true})
}

func githubStatus() map[string]any {
	set := forge.GithubToken() != ""
	return map[string]any{"installed": set, "authed": set, "account": ""}
}
