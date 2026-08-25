package core

import (
	"encoding/base64"
	"net/http"
	"os"
	"sort"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/configbundle"
	"github.com/pomelohq/pomelo/internal/secrets"
)

func (s *Server) configBundleRoutes(mux *http.ServeMux) {
	mux.HandleFunc("/api/config/bundle/export", s.handleBundleExport)
	mux.HandleFunc("/api/config/bundle/read", s.handleBundleRead)
	mux.HandleFunc("/api/config/bundle/apply", s.handleBundleApply)
	mux.HandleFunc("/api/config/bundle/adapt", s.handleBundleAdapt)
}

func (s *Server) handleBundleAdapt(w http.ResponseWriter, r *http.Request) {
	req, err := readJSON[struct {
		Data          string `json:"data"`
		Password      string `json:"password"`
		YAML          string `json:"yaml"`
		CreateSecrets bool   `json:"create_secrets"`
	}](r)
	if err != nil {
		httpErr(w, http.StatusBadRequest, "bad json")
		return
	}
	writeJSON(w, s.BundleAdapt(req.Data, req.Password, req.YAML, req.CreateSecrets))
}

func (s *Server) BundleAdapt(dataB64, password, yaml string, createSecrets bool) map[string]any {
	if createSecrets && dataB64 != "" {
		if raw, e := base64.StdEncoding.DecodeString(dataB64); e == nil {
			if _, m, e2 := configbundle.Open(raw, password); e2 == nil {
				for n, v := range m {
					_ = secrets.Set(s.session(), n, v)
				}
			}
		}
	}
	dst := s.WorkspaceRoot + "/pom-import-source.yml"
	if err := os.WriteFile(dst, []byte(yaml), 0o644); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	return map[string]any{"ok": true, "path": dst,
		"prompt": "Merge pom-import-source.yml into my pom config, conforming to the CANONICAL schema (docs/config-schema.md) — pom.yml is config to RUN the project, dot-notation ONLY. Steps: " +
			"(1) map the source's repo names/paths/services/ports onto my project's repos. " +
			"(2) MIGRATE every legacy form — rewrite colon templates to dot ({{conn:x}}→{{shared.x.url}}, {{host:x}}/{{port:x}}→{{shared.x.host}}/.port, {{db:x}}→{{db.x}}, {{user:x}}/{{pass:x}}/{{slot:x}}→{{shared.x.*}}); NEVER keep a colon form (config_validate rejects them). " +
			"(3) DROP anything not part of running the project: `proxy:`/`webhook:` (system auto-routes), `ui`/`code_agents`/`jira` (app settings), `e2e`, `exposes:`+`{{var:}}`, `combinations`/`workspaces`. Don't carry them over. " +
			"(4) replace any real secret literal with {{secret.NAME}} (the values are already in the secret store). " +
			"(5) fold per-repo setup/migrate/seed/shortcuts into `lifecycle.commands`. " +
			"Then loop config_validate until zero errors, call config_normalize as the FINAL step (it deterministically strips removed keys, migrates colon→dot, and tidies into pom.d), and report exactly what you merged, migrated, and dropped. Delete pom-import-source.yml when done."}
}

func (s *Server) handleBundleExport(w http.ResponseWriter, r *http.Request) {
	req, err := readJSON[struct {
		IncludeSecrets bool   `json:"include_secrets"`
		Password       string `json:"password"`
	}](r)
	if err != nil {
		httpErr(w, http.StatusBadRequest, "bad json")
		return
	}
	writeJSON(w, s.BundleExport(req.IncludeSecrets, req.Password))
}

func (s *Server) BundleExport(includeSecrets bool, password string) map[string]any {
	yaml, _, err := config.MergedYAML(s.configPath())
	if err != nil {
		return map[string]any{"error": err.Error()}
	}
	var out []byte
	name := "pom-config.yml"
	if includeSecrets {
		if password == "" {
			return map[string]any{"error": "password required to include secrets"}
		}
		vals := map[string]string{}
		for _, n := range secrets.Names(s.session()) {
			if v, ok := secrets.Get(s.session(), n); ok {
				vals[n] = v
			}
		}
		out, err = configbundle.BuildEncrypted(string(yaml), vals, password)
		if err != nil {
			return map[string]any{"error": err.Error()}
		}
		name = "pom-config.pombundle"
	} else {
		out = configbundle.BuildPlain(string(yaml))
	}
	return map[string]any{"filename": name, "data": base64.StdEncoding.EncodeToString(out)}
}

func (s *Server) handleBundleRead(w http.ResponseWriter, r *http.Request) {
	req, err := readJSON[struct {
		Data     string `json:"data"`
		Password string `json:"password"`
	}](r)
	if err != nil {
		httpErr(w, http.StatusBadRequest, "bad json")
		return
	}
	writeJSON(w, s.BundleRead(req.Data, req.Password))
}

func (s *Server) BundleRead(dataB64, password string) map[string]any {
	raw, err := base64.StdEncoding.DecodeString(dataB64)
	if err != nil {
		return map[string]any{"error": "bad base64"}
	}
	encrypted := configbundle.IsEncrypted(raw)
	if encrypted && password == "" {
		return map[string]any{"encrypted": true, "need_password": true}
	}
	yaml, secretMap, err := configbundle.Open(raw, password)
	if err != nil {
		return map[string]any{"error": err.Error()}
	}
	names := make([]string, 0, len(secretMap))
	for n := range secretMap {
		names = append(names, n)
	}
	sort.Strings(names)
	return map[string]any{"encrypted": encrypted, "yaml": yaml, "secret_names": names}
}

func (s *Server) handleBundleApply(w http.ResponseWriter, r *http.Request) {
	req, err := readJSON[struct {
		Data          string `json:"data"`
		Password      string `json:"password"`
		YAML          string `json:"yaml"`
		WriteConfig   bool   `json:"write_config"`
		CreateSecrets bool   `json:"create_secrets"`
	}](r)
	if err != nil {
		httpErr(w, http.StatusBadRequest, "bad json")
		return
	}
	writeJSON(w, s.BundleApply(req.Data, req.Password, req.YAML, req.WriteConfig, req.CreateSecrets))
}

func (s *Server) BundleApply(dataB64, password, yaml string, writeConfig, createSecrets bool) map[string]any {
	split := false
	if writeConfig && yaml != "" {
		path := s.configPath()
		if cur, err := os.ReadFile(path); err == nil {
			_ = os.WriteFile(path+".bak", cur, 0o644)
		}
		if err := os.WriteFile(path, []byte(yaml), 0o644); err != nil {
			return map[string]any{"ok": false, "error": err.Error()}
		}
		if _, err := config.SplitToFragments(path, false); err == nil {
			split = true
		}
	}
	created := 0
	if createSecrets {
		raw, err := base64.StdEncoding.DecodeString(dataB64)
		if err != nil {
			return map[string]any{"ok": false, "error": "bad base64"}
		}
		_, secretMap, err := configbundle.Open(raw, password)
		if err != nil {
			return map[string]any{"ok": false, "error": err.Error()}
		}
		for n, v := range secretMap {
			if secrets.Set(s.session(), n, v) == nil {
				created++
			}
		}
	}
	reloaded := false
	if writeConfig && yaml != "" {
		if r := s.ConfigReload(); r["error"] == nil {
			reloaded = true
		}
	}
	return map[string]any{"ok": true, "secrets_created": created, "reloaded": reloaded, "split": split}
}
