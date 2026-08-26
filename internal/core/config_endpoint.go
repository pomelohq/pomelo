package core

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/services"
	"gopkg.in/yaml.v3"
)

func (s *Server) configPath() string {
	if p, ok := configFileIn(s.WorkspaceRoot); ok {
		return p
	}
	return filepath.Join(s.WorkspaceRoot, "pom.yml")
}

func configFileIn(dir string) (string, bool) { return config.ConfigFileIn(dir) }

func (s *Server) handleConfigRead(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.ConfigRead())
}

func (s *Server) ConfigRead() map[string]any {
	if s.WorkspaceRoot == "" {
		return map[string]any{"yaml": "", "path": "", "split": false, "read_only": false, "files": []cfgFileInfo{}}
	}
	data, split, err := config.MergedYAML(s.configPath())
	if err != nil {
		return map[string]any{"error": err.Error(), "yaml": "", "files": s.configFiles()}
	}
	return map[string]any{
		"path": s.configPath(), "yaml": string(data), "split": split, "read_only": split,
		"files": s.configFiles(),
	}
}

func (s *Server) handleConfigExport(w http.ResponseWriter, r *http.Request) {
	if s.WorkspaceRoot == "" {
		http.Error(w, "no project loaded", http.StatusServiceUnavailable)
		return
	}
	data, split, err := config.MergedYAML(s.configPath())
	if err != nil {
		httpErr(w, http.StatusInternalServerError, "%s", err.Error())
		return
	}
	redact := r.URL.Query().Get("redact") == "true"
	if redact {
		if data, err = config.RedactYAML(data); err != nil {
			httpErr(w, http.StatusInternalServerError, "%s", err.Error())
			return
		}
	}
	writeJSON(w, map[string]any{"yaml": string(data), "redacted": redact, "split": split})
}

func (s *Server) handleConfigImport(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.WorkspaceRoot == "" {
		http.Error(w, "no project loaded", http.StatusServiceUnavailable)
		return
	}
	var req struct {
		YAML  string `json:"yaml"`
		Force bool   `json:"force"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	if err := validateConfigYAML(req.YAML); err != nil {
		writeJSON(w, map[string]any{"ok": false, "error": err.Error()})
		return
	}
	path := s.configPath()
	pomd := filepath.Join(filepath.Dir(path), "pom.d")
	if _, err := osStat(pomd); err == nil && !req.Force {
		writeJSON(w, map[string]any{"ok": false, "split": true,
			"error": "config is split across pom.d/ — importing would be shadowed by the fragments. Enable \"replace split config\" to move pom.d aside."})
		return
	}
	if cur, err := os.ReadFile(path); err == nil {
		_ = os.WriteFile(path+".bak", cur, 0o644)
	}
	if req.Force {
		if _, err := osStat(pomd); err == nil {
			_ = os.Rename(pomd, pomd+".bak")
		}
	}
	if err := os.WriteFile(path, []byte(req.YAML), 0o644); err != nil {
		httpErr(w, http.StatusInternalServerError, "%s", err.Error())
		return
	}
	writeJSON(w, map[string]any{"ok": true, "note": "Imported. Use Reload config to apply."})
}

type configWriteReq struct {
	YAML string `json:"yaml"`
	Dry  bool   `json:"dry"`
}

func (s *Server) handleConfigWrite(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	if s.WorkspaceRoot == "" {
		http.Error(w, "no project loaded", http.StatusServiceUnavailable)
		return
	}
	var req configWriteReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	if !req.Dry {
		if _, split, _ := config.MergedYAML(s.configPath()); split {
			writeJSON(w, map[string]any{"ok": false, "error": "config is split across pom.d/*.yml — edit those files directly (dashboard is read-only when split)"})
			return
		}
	}
	if err := validateConfigYAML(req.YAML); err != nil {
		writeJSON(w, map[string]any{"ok": false, "error": err.Error()})
		return
	}
	if rk := config.RemovedKeysInYAML([]byte(req.YAML)); len(rk) > 0 {
		writeJSON(w, map[string]any{"ok": false, "error": "removed/unsupported keys — delete them (or run config_normalize): " + strings.Join(rk, ", ")})
		return
	}
	if req.Dry {
		writeJSON(w, map[string]any{"ok": true, "dry": true})
		return
	}
	if err := os.WriteFile(s.configPath(), []byte(req.YAML), 0o644); err != nil {
		httpErr(w, http.StatusInternalServerError, "%s", err.Error())
		return
	}
	writeJSON(w, map[string]any{
		"ok":   true,
		"note": "Saved. Use Reload config to apply without restarting.",
	})
}

type cfgFileInfo struct {
	Name string `json:"name"`
	Path string `json:"path"`
	Root bool   `json:"root"`
}

func (s *Server) configFiles() []cfgFileInfo {
	root := s.configPath()
	dir := filepath.Dir(root)
	files := []cfgFileInfo{{Name: filepath.Base(root), Path: root, Root: true}}
	for _, f := range config.FragmentPaths(dir) {
		rel, err := filepath.Rel(dir, f)
		if err != nil {
			rel = filepath.Base(f)
		}
		files = append(files, cfgFileInfo{Name: rel, Path: f})
	}
	return files
}

func (s *Server) configFileAllowed(path string) bool {
	for _, f := range s.configFiles() {
		if f.Path == path {
			return true
		}
	}
	return false
}

func (s *Server) handleConfigFiles(w http.ResponseWriter, r *http.Request) {
	if s.WorkspaceRoot == "" {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	writeJSON(w, map[string]any{"files": s.configFiles()})
}

func (s *Server) handleConfigLocate(w http.ResponseWriter, r *http.Request) {
	if s.WorkspaceRoot == "" {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	section := r.URL.Query().Get("section")
	repo := r.URL.Query().Get("repo")
	key := map[string]string{
		"environments": "environments",
		"presets":      "presets",
		"shared":       "shared_services",
		"repos":        "repos",
		"general":      "",
	}[section]

	root := s.configPath()
	files := s.configFiles()
	if section == "general" {
		s.replyConfigFile(w, root)
		return
	}
	best := root
	for _, f := range files {
		data, err := os.ReadFile(f.Path)
		if err != nil {
			continue
		}
		var doc map[string]any
		if yaml.Unmarshal(data, &doc) != nil {
			continue
		}
		sub, ok := doc[key].(map[string]any)
		if !ok || len(sub) == 0 {
			continue
		}
		if section == "repos" && repo != "" {
			if _, has := sub[repo]; !has {
				continue
			}
		}
		best = f.Path
		break
	}
	s.replyConfigFile(w, best)
}

func (s *Server) replyConfigFile(w http.ResponseWriter, path string) {
	data, err := os.ReadFile(path)
	if err != nil {
		httpErr(w, http.StatusInternalServerError, "%s", err.Error())
		return
	}
	writeJSON(w, map[string]any{"path": path, "yaml": string(data), "name": filepath.Base(path)})
}

func (s *Server) handleConfigFile(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		writeJSON(w, s.ConfigFileGet(r.URL.Query().Get("path")))
	case http.MethodPost:
		req, err := readJSON[struct {
			Path string `json:"path"`
			YAML string `json:"yaml"`
			Dry  bool   `json:"dry"`
		}](r)
		if err != nil {
			httpErr(w, http.StatusBadRequest, "bad json")
			return
		}
		writeJSON(w, s.ConfigFileSet(req.Path, req.YAML, req.Dry))
	default:
		httpErr(w, http.StatusMethodNotAllowed, "GET or POST")
	}
}

func (s *Server) ConfigFileGet(path string) map[string]any {
	if s.WorkspaceRoot == "" {
		return map[string]any{"error": "no project loaded"}
	}
	if !s.configFileAllowed(path) {
		return map[string]any{"error": "unknown config file"}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return map[string]any{"error": err.Error()}
	}
	return map[string]any{"path": path, "yaml": string(data)}
}

func (s *Server) ConfigFileSet(path, yaml string, dry bool) map[string]any {
	if s.WorkspaceRoot == "" {
		return map[string]any{"ok": false, "error": "no project loaded"}
	}
	if !s.configFileAllowed(path) {
		return map[string]any{"ok": false, "error": "unknown config file"}
	}
	if err := validateConfigYAMLSyntax(yaml); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	note := "Saved. Use Reload config to apply."
	if editErr := s.validateConfigWithOverride(path, yaml); editErr != nil {
		if s.validateCurrentConfig() == nil {
			return map[string]any{"ok": false, "error": editErr.Error()}
		}
		note = "Saved (config still has errors elsewhere — keep fixing): " + editErr.Error()
	}
	if dry {
		return map[string]any{"ok": true, "dry": true, "note": note}
	}
	if err := os.WriteFile(path, []byte(yaml), 0o644); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	return map[string]any{"ok": true, "note": note}
}

// ConfigFileCreate makes a new pom.d/<name>.yml fragment. name is relative to
// pom.d (may include subdirs), must stay under it, and must be a .yml/.yaml file
// that doesn't already exist. The merged config is validated before the write so
// a new fragment can't leave the project broken.
func (s *Server) ConfigFileCreate(name, yaml string) map[string]any {
	if s.WorkspaceRoot == "" {
		return map[string]any{"ok": false, "error": "no project loaded"}
	}
	name = strings.TrimSpace(name)
	if name == "" || strings.Contains(name, "..") || strings.HasPrefix(name, "/") {
		return map[string]any{"ok": false, "error": "invalid file name"}
	}
	clean := filepath.Clean(name)
	if ext := filepath.Ext(clean); ext != ".yml" && ext != ".yaml" {
		return map[string]any{"ok": false, "error": "file must end in .yml or .yaml"}
	}
	dir := filepath.Dir(s.configPath())
	pomd := filepath.Join(dir, "pom.d")
	path := filepath.Join(pomd, clean)
	if !strings.HasPrefix(path, pomd+string(filepath.Separator)) {
		return map[string]any{"ok": false, "error": "path escapes pom.d"}
	}
	if _, err := os.Stat(path); err == nil {
		return map[string]any{"ok": false, "error": "file already exists"}
	}
	if yaml == "" {
		yaml = "# " + clean + " — merged into pom.yml. Add repos:, shared_services:, env:, etc.\n"
	}
	if err := validateConfigYAMLSyntax(yaml); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	if editErr := s.validateConfigWithExtra(path, yaml); editErr != nil {
		if s.validateCurrentConfig() == nil {
			return map[string]any{"ok": false, "error": editErr.Error()}
		}
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	if err := os.WriteFile(path, []byte(yaml), 0o644); err != nil {
		return map[string]any{"ok": false, "error": err.Error()}
	}
	return map[string]any{"ok": true, "path": path, "name": filepath.Base(path)}
}

// validateConfigWithExtra validates the merged config as if a new fragment
// (extraPath) with extraBody were already present.
func (s *Server) validateConfigWithExtra(extraPath, extraBody string) error {
	root := s.configPath()
	dir := filepath.Dir(root)
	tmp, err := os.MkdirTemp("", "pom-cfg-mirror-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(tmp)
	write := func(src string, body []byte) error {
		rel, err := filepath.Rel(dir, src)
		if err != nil {
			return err
		}
		dst := filepath.Join(tmp, rel)
		if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
			return err
		}
		return os.WriteFile(dst, body, 0o644)
	}
	for _, f := range s.configFiles() {
		body, err := os.ReadFile(f.Path)
		if err != nil {
			return err
		}
		if err := write(f.Path, body); err != nil {
			return err
		}
	}
	if err := write(extraPath, []byte(extraBody)); err != nil {
		return err
	}
	cfg, err := config.Load(filepath.Join(tmp, filepath.Base(root)))
	if err != nil {
		return err
	}
	return cfg.Validate()
}

func (s *Server) validateConfigWithOverride(target, yamlBody string) error {
	root := s.configPath()
	dir := filepath.Dir(root)
	tmp, err := os.MkdirTemp("", "pom-cfg-mirror-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(tmp)

	mirror := func(src string) error {
		rel, err := filepath.Rel(dir, src)
		if err != nil {
			return err
		}
		dst := filepath.Join(tmp, rel)
		if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
			return err
		}
		body, err := os.ReadFile(src)
		if err != nil {
			return err
		}
		if src == target {
			body = []byte(yamlBody)
		}
		return os.WriteFile(dst, body, 0o644)
	}
	for _, f := range s.configFiles() {
		if err := mirror(f.Path); err != nil {
			return err
		}
	}
	cfg, err := config.Load(filepath.Join(tmp, filepath.Base(root)))
	if err != nil {
		return err
	}
	return cfg.Validate()
}

func (s *Server) validateCurrentConfig() error {
	cfg, err := config.Load(s.configPath())
	if err != nil {
		return err
	}
	return cfg.Validate()
}

func validateConfigYAMLSyntax(y string) error {
	var sink any
	if err := yaml.Unmarshal([]byte(y), &sink); err != nil {
		return fmt.Errorf("yaml parse error: %w", err)
	}
	return nil
}

func validateConfigYAML(y string) error {
	var sink any
	if err := yaml.Unmarshal([]byte(y), &sink); err != nil {
		return fmt.Errorf("yaml parse error: %w", err)
	}
	tmp, err := os.CreateTemp("", "pom-cfg-*.yml")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	if _, err := tmp.WriteString(y); err != nil {
		tmp.Close()
		return err
	}
	tmp.Close()
	cfg, err := config.Load(tmp.Name())
	if err != nil {
		return err
	}
	return cfg.Validate()
}

func (s *Server) handleConfigReload(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	resp := s.ConfigReload()
	if err, ok := resp["error"].(string); ok {
		httpErr(w, http.StatusBadRequest, "%s", err)
		return
	}
	writeJSON(w, resp)
}

func (s *Server) ConfigReload() map[string]any {
	if s.WorkspaceRoot == "" {
		return map[string]any{"error": "no project loaded"}
	}
	cfg, err := config.Load(s.configPath())
	if err != nil {
		return map[string]any{"error": err.Error()}
	}
	s.setCfg(cfg)
	s.Project = cfg.Session
	s.DefaultBranch = cfg.GlobalDefaultBranch()

	services.InitNetwork(s.WorkspaceRoot, cfg.Session, cfg)

	known := map[string]bool{}
	for n := range cfg.Repos {
		known[n] = true
	}
	regenerated := 0
	for _, ws := range scanWorkspaces(s.WorkspaceRoot, s.DefaultBranch, known, true) {
		services.RegenerateWorkspaceEnv(s.WorkspaceRoot, cfg, ws.Branch)
		regenerated++
	}

	restarted := s.restartStaleServices()

	s.startDevProxy()

	return map[string]any{
		"ok":          true,
		"project":     s.Project,
		"regenerated": regenerated,
		"restarted":   restarted,
		"proxy":       s.devProxyListening(),
	}
}

func (s *Server) handleConfigSplit(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	if s.WorkspaceRoot == "" {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	dry := r.URL.Query().Get("dry") == "true"
	res, err := config.SplitToFragments(s.configPath(), dry)
	if err != nil {
		writeJSON(w, map[string]any{"ok": false, "error": err.Error()})
		return
	}
	if !dry {
		s.ConfigReload()
	}
	writeJSON(w, map[string]any{"ok": true, "fragments": res.Fragments, "backup": res.BackupFile})
}

func (s *Server) handleConfigNormalize(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	if s.WorkspaceRoot == "" {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	removed, err := config.Normalize(s.configPath())
	if err != nil {
		writeJSON(w, map[string]any{"ok": false, "error": err.Error()})
		return
	}
	s.ConfigReload()
	writeJSON(w, map[string]any{"ok": true, "removed": removed})
}

func (s *Server) handleConfigVariables(w http.ResponseWriter, r *http.Request) {
	cfg := s.cfg()
	if cfg == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	branch := r.URL.Query().Get("branch")
	if branch == "" {
		branch = cfg.GlobalDefaultBranch()
	}
	envs := []string{}
	for name := range cfg.Environments {
		if name != "local" {
			envs = append(envs, name)
		}
	}
	sort.Strings(envs)
	writeJSON(w, map[string]any{
		"branch":       branch,
		"environments": envs,
		"variables":    []any{},
	})
}

func (s *Server) handleConfigEnvOverride(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	if s.WorkspaceRoot == "" {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	req, err := readJSON[struct {
		Profile string `json:"profile"`
		Key     string `json:"key"`
		Value   string `json:"value"`
		Unset   bool   `json:"unset"`
	}](r)
	if err != nil || req.Profile == "" || req.Key == "" {
		httpErr(w, http.StatusBadRequest, "profile and key required")
		return
	}
	path := s.configPath()
	if req.Unset {
		err = config.UnsetEnvOverride(path, req.Profile, req.Key)
	} else {
		err = config.SetEnvOverride(path, req.Profile, req.Key, req.Value)
	}
	if err != nil {
		httpErr(w, http.StatusInternalServerError, "%v", err)
		return
	}
	cfg, err := config.Load(path)
	if err != nil {
		httpErr(w, http.StatusInternalServerError, "reload: %v", err)
		return
	}
	s.setCfg(cfg)
	writeJSON(w, map[string]any{"ok": true})
}

func (s *Server) handleConfigRepoEnv(w http.ResponseWriter, r *http.Request) {
	cfg := s.cfg()
	if cfg == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	if r.Method == http.MethodPost {
		req, err := readJSON[struct {
			Repo  string `json:"repo"`
			Key   string `json:"key"`
			Value string `json:"value"`
			Unset bool   `json:"unset"`
		}](r)
		if err != nil || req.Repo == "" || req.Key == "" {
			httpErr(w, http.StatusBadRequest, "repo and key required")
			return
		}
		if req.Unset {
			err = config.UnsetRepoEnv(s.configPath(), req.Repo, []string{req.Key})
		} else {
			err = config.SetRepoEnv(s.configPath(), req.Repo, map[string]string{req.Key: req.Value})
		}
		if err != nil {
			httpErr(w, http.StatusInternalServerError, "%v", err)
			return
		}
		if reloaded, err := config.Load(s.configPath()); err == nil {
			s.setCfg(reloaded)
		}
		writeJSON(w, map[string]any{"ok": true})
		return
	}
	repo := r.URL.Query().Get("repo")
	if repo == "" {
		httpErr(w, http.StatusBadRequest, "repo required")
		return
	}
	branch := r.URL.Query().Get("branch")
	if branch == "" {
		branch = cfg.GlobalDefaultBranch()
	}
	dir := cfg.Repos[repo]
	if dir == nil {
		for _, d := range cfg.Repos {
			if d.Alias == repo {
				dir = d
				break
			}
		}
	}
	if dir == nil {
		httpErr(w, http.StatusNotFound, "no repo %q", repo)
		return
	}
	svc := ""
	if len(dir.ServiceOrder) > 0 {
		svc = dir.ServiceOrder[0]
	}
	se, err := services.ExplainService(cfg, repo, svc, branch, r.URL.Query().Get("env"))
	if err != nil {
		httpErr(w, http.StatusInternalServerError, "%v", err)
		return
	}
	dups := map[string][]string{}
	if all, err := config.AnalyzeDuplicateEnv(s.configPath()); err == nil {
		for _, d := range all {
			for _, rn := range d.Repos {
				if rn == se.Repo {
					dups[d.Key] = d.Repos
					break
				}
			}
		}
	}
	writeJSON(w, map[string]any{
		"repo":    se.Repo,
		"alias":   se.Alias,
		"presets": dir.Presets_,
		"env":     se.Env,
		"dups":    dups,
	})
}

func (s *Server) handleConfigExplain(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	writeJSON(w, s.ConfigExplain(q.Get("repo"), q.Get("branch"), q.Get("svc"), q.Get("env")))
}

func (s *Server) ConfigExplain(repo, branch, svc, env string) map[string]any {
	cfg := s.cfg()
	if cfg == nil {
		return map[string]any{"error": "no project loaded"}
	}
	type svcRef struct {
		Repo     string   `json:"repo"`
		Alias    string   `json:"alias"`
		Services []string `json:"services"`
	}
	repos := make([]svcRef, 0, len(cfg.RepoOrder))
	for _, name := range cfg.RepoOrder {
		dir := cfg.Repos[name]
		if dir == nil || len(dir.ServiceOrder) == 0 {
			continue
		}
		alias := dir.Alias
		if alias == "" {
			alias = name
		}
		repos = append(repos, svcRef{Repo: name, Alias: alias, Services: dir.ServiceOrder})
	}
	if repo == "" {
		return map[string]any{"repos": repos}
	}
	if branch == "" {
		branch = cfg.GlobalDefaultBranch()
	}
	if svc == "" {
		if dir := cfg.Repos[repo]; dir != nil && len(dir.ServiceOrder) > 0 {
			svc = dir.ServiceOrder[0]
		}
	}
	se, err := services.ExplainService(cfg, repo, svc, branch, env)
	if err != nil {
		return map[string]any{"repos": repos, "error": err.Error()}
	}
	return map[string]any{"repos": repos, "explain": se}
}

func (s *Server) handleConfigHealth(w http.ResponseWriter, r *http.Request) {
	if s.WorkspaceRoot == "" {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	path := s.configPath()
	redundant, _ := config.LintRedundantDefaults(path)
	legacy, _ := config.LintLegacyTokens(path)
	depKeys, _ := config.LintDeprecatedKeys(path)
	dups, _ := config.AnalyzeDuplicateEnv(path)
	writeJSON(w, map[string]any{
		"redundant_defaults": redundant,
		"legacy_tokens":      legacy,
		"deprecated_keys":    depKeys,
		"duplicated_env":     dups,
	})
}

func (s *Server) handleConfigMigrateTokens(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	path := s.configPath()
	changes, err := config.MigrateTokens(path, true)
	if err != nil {
		httpErr(w, http.StatusInternalServerError, "%v", err)
		return
	}
	if cfg, err := config.Load(path); err == nil {
		s.setCfg(cfg)
	}
	writeJSON(w, map[string]any{"ok": true, "files": len(changes)})
}

func (s *Server) handleConfigExtractPreset(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	req, err := readJSON[struct {
		Into string   `json:"into"`
		Keys []string `json:"keys"`
	}](r)
	if err != nil || req.Into == "" || len(req.Keys) == 0 {
		httpErr(w, http.StatusBadRequest, "into and keys required")
		return
	}
	path := s.configPath()
	plan, err := config.PlanExtractPreset(path, req.Into, req.Keys)
	if err != nil {
		httpErr(w, http.StatusBadRequest, "%v", err)
		return
	}
	if err := config.ApplyExtractPreset(path, plan); err != nil {
		httpErr(w, http.StatusInternalServerError, "%v", err)
		return
	}
	if cfg, err := config.Load(path); err == nil {
		s.setCfg(cfg)
	}
	writeJSON(w, map[string]any{"ok": true, "repos": plan.Repos})
}

func (s *Server) handleLifecycle(w http.ResponseWriter, r *http.Request) {
	cfg := s.cfg()
	if cfg == nil {
		httpErr(w, http.StatusServiceUnavailable, "no project loaded")
		return
	}
	repo := r.URL.Query().Get("repo")
	if repo == "" {
		httpErr(w, http.StatusBadRequest, "repo required")
		return
	}
	lv, ok := services.LifecycleOf(cfg, repo)
	if !ok {
		httpErr(w, http.StatusNotFound, "no repo %q", repo)
		return
	}
	writeJSON(w, lv)
}
