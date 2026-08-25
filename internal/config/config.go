package config

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

type Config struct {
	Session        string                       `yaml:"session"`
	DefaultBranch  string                       `yaml:"default_branch"`
	Repos          map[string]*Dir              `yaml:"repos"`
	Presets        map[string]*PresetConfig     `yaml:"presets"`
	SharedServices map[string]*SharedServiceDef `yaml:"shared_services"`
	Workspaces     map[string][]string          `yaml:"workspaces"`
	Combinations   map[string][]string          `yaml:"combinations"`
	Environments   map[string]map[string]string `yaml:"environments"`
	CodeAgents     *CodeAgentsConfig            `yaml:"code_agents"`
	UI             *UIConfig                    `yaml:"ui"`
	Sync           *SyncConfig                  `yaml:"sync"`
	Seed           []string                     `yaml:"seed"`
	PrepareMain    []string                     `yaml:"prepare_main"`
	RawPreset      yaml.Node                    `yaml:"preset"`
	Plugins        map[string]yaml.Node         `yaml:"plugins"`

	RepoOrder      []string            `yaml:"-"`
	SharedOrder    []string            `yaml:"-"`
	WsServices     map[string]*Service `yaml:"-"`
	WsServiceOrder []string            `yaml:"-"`
}

type Dir struct {
	Alias         string              `yaml:"alias"`
	PreStart      string              `yaml:"pre_start"`
	ShellEnv      string              `yaml:"shell_env"`
	DefaultBranch string              `yaml:"default_branch"`
	Shortcuts     []Shortcut          `yaml:"shortcuts"`
	Services      map[string]*Service `yaml:"services"`
	ProxyPort     *uint16             `yaml:"proxy_port"`
	Profiles      StringList          `yaml:"profiles"`

	Copy          []string             `yaml:"copy"`
	EnvOutput     []EnvFileEntry       `yaml:"-"`
	Env           map[string]string    `yaml:"-"`
	RawEnv        yaml.Node            `yaml:"env"`
	SharedSvcRefs []SharedServiceRef   `yaml:"-"`
	RawSharedSvcs yaml.Node            `yaml:"shared_services"`
	Databases     map[string]string    `yaml:"databases"`
	Preset        yaml.Node            `yaml:"preset"`
	Presets_      []string             `yaml:"-"`
	Setup         []string             `yaml:"setup"`
	Migrate       []string             `yaml:"migrate"`
	Seed          []string             `yaml:"seed"`
	SeedFromMain  bool                 `yaml:"seed_from_main"`
	Commands      map[string]string    `yaml:"commands"`
	PreDelete     []string             `yaml:"pre_delete"`
	Plugins       map[string]yaml.Node `yaml:"plugins"`
	Lifecycle     *LifecycleConfig     `yaml:"lifecycle"`

	ServiceOrder []string `yaml:"-"`
}

type LifecycleConfig struct {
	Copy      []string          `yaml:"copy"`
	Setup     []string          `yaml:"setup"`
	Migrate   []string          `yaml:"migrate"`
	Seed      []string          `yaml:"seed"`
	PreDelete []string          `yaml:"pre_delete"`
	PreStart  string            `yaml:"pre_start"`
	Shortcuts []Shortcut        `yaml:"shortcuts"`
	Commands  map[string]string `yaml:"commands"`
}

func (d *Dir) foldLifecycle() {
	lc := d.Lifecycle
	if lc == nil {
		return
	}
	if len(lc.Copy) > 0 {
		d.Copy = lc.Copy
	}
	if len(lc.Setup) > 0 {
		d.Setup = lc.Setup
	}
	if len(lc.Migrate) > 0 {
		d.Migrate = lc.Migrate
	}
	if len(lc.Seed) > 0 {
		d.Seed = lc.Seed
	}
	if len(lc.PreDelete) > 0 {
		d.PreDelete = lc.PreDelete
	}
	if lc.PreStart != "" {
		d.PreStart = lc.PreStart
	}
	if len(lc.Shortcuts) > 0 {
		d.Shortcuts = lc.Shortcuts
	}
	if len(lc.Commands) > 0 {
		d.Commands = lc.Commands
	}
}

func titleCase(s string) string {
	if s == "" {
		return s
	}
	return strings.ToUpper(s[:1]) + s[1:]
}

var lifecycleSetupOrder = []string{"install", "generate", "migrate"}

var lifecycleDesc = map[string]string{
	"install": "Install dependencies", "generate": "Generate types",
	"migrate": "Migrate database", "test": "Test", "lint": "Lint",
	"build": "Build", "format": "Format",
}

func (d *Dir) EffectiveSetup() []string {
	if len(d.Setup) > 0 {
		return d.Setup
	}
	var out []string
	for _, k := range lifecycleSetupOrder {
		if c := d.Commands[k]; c != "" {
			out = append(out, c)
		}
	}
	return out
}

func (d *Dir) EffectiveMigrate() []string {
	if len(d.Migrate) > 0 {
		return d.Migrate
	}
	if c := d.Commands["migrate"]; c != "" {
		return []string{c}
	}
	return nil
}

func (d *Dir) EffectiveShortcuts() []Shortcut {
	out := append([]Shortcut{}, d.Shortcuts...)
	seen := map[string]bool{}
	for _, s := range out {
		seen[s.Cmd] = true
	}
	order := []string{"install", "generate", "migrate", "test", "lint", "build", "format"}
	for k := range d.Commands {
		known := false
		for _, o := range order {
			if o == k {
				known = true
				break
			}
		}
		if !known {
			order = append(order, k)
		}
	}
	sort.Strings(order[7:])
	for _, k := range order {
		c := d.Commands[k]
		if c == "" || seen[c] {
			continue
		}
		seen[c] = true
		desc := lifecycleDesc[k]
		if desc == "" {
			desc = titleCase(k)
		}
		out = append(out, Shortcut{Desc: desc, Cmd: c, Key: k})
	}
	return out
}

type StringList []string

func (s *StringList) UnmarshalYAML(n *yaml.Node) error {
	if n.Kind == yaml.ScalarNode {
		if n.Value == "" {
			*s = nil
		} else {
			*s = []string{n.Value}
		}
		return nil
	}
	var arr []string
	if err := n.Decode(&arr); err != nil {
		return err
	}
	*s = arr
	return nil
}

type EnvFileEntry struct {
	File string
	Env  map[string]string
}

type UIConfig struct {
	Editor string `yaml:"editor"`
}

type SyncConfig struct {
	AutoPush           bool `yaml:"auto_push"`
	IntervalSec        int  `yaml:"interval_sec"`
	RefreshMain        bool `yaml:"refresh_main"`
	RefreshIntervalSec int  `yaml:"refresh_interval_sec"`
}

type CodeAgentsConfig struct {
	Disabled       bool     `yaml:"disabled"`
	Only           []string `yaml:"only"`
	NotifyDisabled bool     `yaml:"notify_disabled"`
}

type PresetConfig struct {
	Preset       yaml.Node `yaml:"preset"`
	presets_     []string
	Env          map[string]string   `yaml:"env"`
	Setup        []string            `yaml:"setup"`
	Seed         []string            `yaml:"seed"`
	PreDelete    []string            `yaml:"pre_delete"`
	PreStart     string              `yaml:"pre_start"`
	Copy         []string            `yaml:"copy"`
	SeedFromMain bool                `yaml:"seed_from_main"`
	Shortcuts    []Shortcut          `yaml:"shortcuts"`
	Services     map[string]*Service `yaml:"services"`
	Commands     map[string]string   `yaml:"commands"`
	Migrate      []string            `yaml:"migrate"`
}

type Service struct {
	Type      string            `yaml:"type"`
	Cmd       string            `yaml:"cmd"`
	Dir       string            `yaml:"dir"`
	ShellEnv  string            `yaml:"shell_env"`
	Env       map[string]string `yaml:"env"`
	PreStart  string            `yaml:"pre_start"`
	ProxyPort *uint16           `yaml:"proxy_port"`
	Shortcuts []Shortcut        `yaml:"shortcuts"`
	DependsOn []string          `yaml:"depends_on"`
	Port      *bool             `yaml:"port"`
	Modes     map[string]string `yaml:"modes"`
	Mode      string            `yaml:"mode"`
	Profiles  StringList        `yaml:"profiles"`
}

func (s *Service) ActiveCmd(modeOverride string) string {
	mode := modeOverride
	if mode == "" {
		mode = s.Mode
	}
	if mode != "" && s.Modes != nil {
		if cmd, ok := s.Modes[mode]; ok {
			return cmd
		}
	}
	return s.Cmd
}

func (s *Service) ModeNames() []string {
	if len(s.Modes) == 0 {
		return nil
	}
	names := make([]string, 0, len(s.Modes))
	for k := range s.Modes {
		names = append(names, k)
	}
	sort.Strings(names)
	return names
}

func (s *Service) HasPort() bool {
	if s.Port != nil {
		return *s.Port
	}
	return s.Type == "backend" || s.Type == "frontend"
}

func (s *Service) UnmarshalYAML(value *yaml.Node) error {
	if value.Kind == yaml.ScalarNode {
		s.Cmd = value.Value
		return nil
	}
	type serviceAlias Service
	var alias serviceAlias
	if err := value.Decode(&alias); err != nil {
		return err
	}
	*s = Service(alias)
	return nil
}

type SharedServiceDef struct {
	Type        string            `yaml:"type"`
	Image       string            `yaml:"image"`
	Host        string            `yaml:"host"`
	Ports       []string          `yaml:"ports"`
	Environment map[string]string `yaml:"environment"`
	Volumes     []string          `yaml:"volumes"`
	Command     string            `yaml:"command"`
	Healthcheck *HealthCheck      `yaml:"healthcheck"`
	DBUser      string            `yaml:"db_user"`
	DBPassword  string            `yaml:"db_password"`
	Capacity    *uint16           `yaml:"capacity"`
}

type HealthCheck struct {
	Test     yaml.Node `yaml:"test"`
	Interval string    `yaml:"interval"`
	Timeout  string    `yaml:"timeout"`
	Retries  int       `yaml:"retries"`
}

type SharedServiceRef struct {
	Name   string
	DBName string
}

type Shortcut struct {
	Cmd  string `yaml:"cmd"`
	Desc string `yaml:"desc"`
	Key  string `yaml:"key,omitempty"`
}

type ResolvedService struct {
	Cmd      string
	WorkDir  string
	Env      string
	PreStart string
}

func (c *Config) PrepareMainPhases() []string {
	if len(c.PrepareMain) == 0 {
		return []string{"reset", "migrate", "seed"}
	}
	known := map[string]bool{"reset": true, "migrate": true, "seed": true}
	out := make([]string, 0, len(c.PrepareMain))
	for _, p := range c.PrepareMain {
		if known[p] {
			out = append(out, p)
		}
	}
	if len(out) == 0 {
		return []string{"reset"}
	}
	return out
}

func (c *Config) GlobalDefaultBranch() string {
	if c.DefaultBranch != "" {
		return c.DefaultBranch
	}
	return "main"
}

func (c *Config) DefaultBranchFor(repoName string) string {
	if dir, ok := c.Repos[repoName]; ok && dir.DefaultBranch != "" {
		return dir.DefaultBranch
	}
	return c.GlobalDefaultBranch()
}

func (c *Config) SharedHost(serviceName string) string {
	return "localhost"
}

func (c *Config) ValidateEnvironment(envName string) error {
	if envName == "" {
		return nil
	}
	if c.Environments == nil || c.Environments[envName] == nil {
		var names []string
		for n := range c.Environments {
			names = append(names, n)
		}
		if len(names) == 0 {
			return fmt.Errorf("environment '%s' not found (no environments defined)", envName)
		}
		return fmt.Errorf("environment '%s' not found (available: %s)", envName, strings.Join(names, ", "))
	}
	return nil
}

func (c *Config) EnvironmentNames() []string {
	var names []string
	for n := range c.Environments {
		names = append(names, n)
	}
	return names
}

func (c *Config) AllServicesFor(dirName string) []string {
	var svcs []string
	if dir, ok := c.Repos[dirName]; ok {
		svcs = append(svcs, dir.ServiceOrder...)
	}
	return svcs
}

func (c *Config) AllWorkspaces() map[string][]string {
	result := make(map[string][]string)
	for k, v := range c.Workspaces {
		result[k] = v
	}
	for k, v := range c.Combinations {
		if _, exists := result[k]; !exists {
			result[k] = v
		}
	}
	if len(result) == 0 {
		var entries []string
		for _, dirName := range c.RepoOrder {
			dir := c.Repos[dirName]
			alias := dir.Alias
			for _, svcName := range dir.ServiceOrder {
				if alias == "" {
					entries = append(entries, svcName)
				} else {
					entries = append(entries, alias+"/"+svcName)
				}
			}
		}
		if len(entries) > 0 {
			result[c.Session] = entries
		}
	}
	return result
}

func (c *Config) FindServiceEntry(entry string) (dirName, svcName string, err error) {
	if prefix, svc, ok := strings.Cut(entry, "/"); ok {
		for dn, dir := range c.Repos {
			matches := dn == prefix || dir.Alias == prefix
			if matches {
				if _, exists := dir.Services[svc]; exists {
					return dn, svc, nil
				}
			}
		}
		return "", "", fmt.Errorf("service '%s' not found", entry)
	}

	var matches []string
	for dn, dir := range c.Repos {
		if _, exists := dir.Services[entry]; exists {
			matches = append(matches, dn)
		}
	}
	switch len(matches) {
	case 0:
		return "", "", fmt.Errorf("service '%s' not found in any dir", entry)
	case 1:
		return matches[0], entry, nil
	default:
		return "", "", fmt.Errorf("ambiguous service '%s' — found in: %s", entry, strings.Join(matches, ", "))
	}
}

func (c *Config) FindServiceEntryQuiet(entry string) (string, string, bool) {
	d, s, err := c.FindServiceEntry(entry)
	return d, s, err == nil
}

func (c *Config) ResolveServices(target string) ([][2]string, error) {
	all := c.AllWorkspaces()
	if entries, ok := all[target]; ok {
		var result [][2]string
		for _, entry := range entries {
			d, s, err := c.FindServiceEntry(entry)
			if err != nil {
				return nil, err
			}
			result = append(result, [2]string{d, s})
		}
		return result, nil
	}

	if dir, ok := c.Repos[target]; ok && len(dir.Services) > 0 {
		var result [][2]string
		for _, svc := range dir.ServiceOrder {
			result = append(result, [2]string{target, svc})
		}
		return result, nil
	}

	for dn, dir := range c.Repos {
		if dir.Alias == target && len(dir.Services) > 0 {
			var result [][2]string
			for _, svc := range dir.ServiceOrder {
				result = append(result, [2]string{dn, svc})
			}
			return result, nil
		}
	}

	d, s, err := c.FindServiceEntry(target)
	if err != nil {
		return nil, err
	}
	return [][2]string{{d, s}}, nil
}

func (c *Config) ResolveService(configDir, dirName, svcName string) (*ResolvedService, error) {
	dir, ok := c.Repos[dirName]
	if !ok {
		return nil, fmt.Errorf("dir '%s' not found", dirName)
	}
	svc, ok := dir.Services[svcName]
	if !ok {
		return nil, fmt.Errorf("service '%s' not found in dir '%s'", svcName, dirName)
	}
	activeCmd := svc.ActiveCmd("")
	if activeCmd == "" {
		return nil, fmt.Errorf("service '%s/%s' has no 'cmd'", dirName, svcName)
	}

	workDir := dirName
	if !filepath.IsAbs(dirName) {
		wsPath := filepath.Join(configDir, fmt.Sprintf("workspace--%s", c.GlobalDefaultBranch()), dirName)
		if info, err := os.Stat(wsPath); err == nil && info.IsDir() {
			workDir = wsPath
		} else {
			workDir = filepath.Join(configDir, dirName)
		}
	}

	if svc.Dir != "" {
		workDir = filepath.Join(workDir, svc.Dir)
	}

	env := svc.ShellEnv
	if env == "" {
		env = dir.ShellEnv
	}
	preStart := svc.PreStart
	if preStart == "" {
		preStart = dir.PreStart
	}

	return &ResolvedService{
		Cmd:      activeCmd,
		WorkDir:  workDir,
		Env:      env,
		PreStart: preStart,
	}, nil
}

func (d *Dir) EnvProfiles(svc *Service) []string {
	list := d.Profiles
	if svc != nil && len(svc.Profiles) > 0 {
		list = svc.Profiles
	}
	out := make([]string, 0, len(list)+1)
	out = append(out, "local")
	for _, p := range list {
		if p != "local" {
			out = append(out, p)
		}
	}
	return out
}

func (d *Dir) OwnEnv() map[string]string {
	base, _ := parseEnv(&d.RawEnv)
	return base
}

func (d *Dir) EnvFileEntries() []EnvFileEntry {
	if len(d.EnvOutput) == 0 {
		return []EnvFileEntry{{File: ".env.local"}}
	}
	return d.EnvOutput
}

func (d *Dir) HasWorktreeConfig() bool {
	return len(d.Copy) > 0 || len(d.Setup) > 0 || len(d.Seed) > 0 || len(d.Databases) > 0 ||
		len(d.Presets_) > 0 || len(d.Commands) > 0 ||
		len(d.PreDelete) > 0 || len(d.Env) > 0 || len(d.EnvOutput) > 0 ||
		len(d.SharedSvcRefs) > 0
}

var postLoadHooks []func(*Config)

func RegisterPostLoadHook(fn func(*Config)) {
	postLoadHooks = append(postLoadHooks, fn)
}

func Load(path string) (*Config, error) {
	data, err := loadMergedYAML(path)
	if err != nil {
		return nil, err
	}

	var raw yaml.Node
	if err := yaml.Unmarshal(data, &raw); err != nil {
		return nil, fmt.Errorf("failed to parse %s: %w", path, err)
	}

	cfg := &Config{
		Session: "pomelo",
		Repos:   make(map[string]*Dir),
	}
	if err := yaml.Unmarshal(data, cfg); err != nil {
		return nil, fmt.Errorf("failed to parse %s: %w", path, err)
	}
	if cfg.Session == "" {
		cfg.Session = "pomelo"
	}

	extractRepoOrder(cfg, &raw)

	for _, dir := range cfg.Repos {
		dir.foldLifecycle()
		dir.Env, dir.EnvOutput = parseEnv(&dir.RawEnv)
		dir.SharedSvcRefs = parseSharedRefs(&dir.RawSharedSvcs)
		dir.Presets_ = parsePresetField(&dir.Preset)
		if dir.Services == nil {
			dir.Services = make(map[string]*Service)
		}
	}

	applyWellKnownDefaults(cfg)
	cfg.applyPresets()
	cfg.applyWsPresets()

	for _, hook := range postLoadHooks {
		hook(cfg)
	}

	return cfg, nil
}

func (c *Config) applyWsPresets() {
	names := parsePresetField(&c.RawPreset)
	c.WsServices = make(map[string]*Service)
	for _, name := range names {
		preset, ok := c.Presets[name]
		if !ok {
			continue
		}
		for svcName, svc := range preset.Services {
			if _, exists := c.WsServices[svcName]; !exists {
				c.WsServices[svcName] = svc
				c.WsServiceOrder = append(c.WsServiceOrder, svcName)
			}
		}
	}
}

const ConfigFileName = "pom.yml"

func ConfigFileIn(dir string) (string, bool) {
	p := filepath.Join(dir, ConfigFileName)
	if info, err := os.Stat(p); err == nil && !info.IsDir() {
		return p, true
	}
	return "", false
}

func FindConfig() (string, error) {
	dir, err := os.Getwd()
	if err != nil {
		return "", err
	}
	return FindConfigFrom(dir)
}

func FindConfigFrom(dir string) (string, error) {
	for {
		candidate := filepath.Join(dir, ConfigFileName)
		if info, err := os.Stat(candidate); err == nil && !info.IsDir() {
			return candidate, nil
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", fmt.Errorf("no %s found (searched from %s to /)", ConfigFileName, dir)
		}
		dir = parent
	}
}

func (c *Config) applyPresets() {
	for name := range c.Presets {
		c.Presets[name].presets_ = parsePresetField(&c.Presets[name].Preset)
	}
	for _, dir := range c.Repos {
		for _, presetName := range c.flattenPresets(dir.Presets_, nil) {
			if preset, ok := c.Presets[presetName]; ok {
				applyPreset(dir, preset)
			}
		}
	}
}

func (c *Config) flattenPresets(names []string, seen map[string]bool) []string {
	if seen == nil {
		seen = map[string]bool{}
	}
	var out []string
	for _, n := range names {
		if seen[n] {
			continue
		}
		seen[n] = true
		if p, ok := c.Presets[n]; ok && len(p.presets_) > 0 {
			out = append(out, c.flattenPresets(p.presets_, seen)...)
		}
		out = append(out, n)
	}
	return out
}

func applyPreset(dir *Dir, preset *PresetConfig) {
	for k, v := range preset.Env {
		if _, exists := dir.Env[k]; !exists {
			if dir.Env == nil {
				dir.Env = make(map[string]string)
			}
			dir.Env[k] = v
		}
	}
	if len(dir.Setup) == 0 {
		dir.Setup = preset.Setup
	}
	if len(dir.Seed) == 0 {
		dir.Seed = preset.Seed
	}
	if len(dir.PreDelete) == 0 {
		dir.PreDelete = preset.PreDelete
	}
	if dir.PreStart == "" {
		dir.PreStart = preset.PreStart
	}
	if len(dir.Copy) == 0 {
		dir.Copy = preset.Copy
	}
	if !dir.SeedFromMain {
		dir.SeedFromMain = preset.SeedFromMain
	}
	if len(dir.Shortcuts) == 0 {
		dir.Shortcuts = preset.Shortcuts
	}
	if len(dir.Migrate) == 0 {
		dir.Migrate = preset.Migrate
	}
	for k, v := range preset.Commands {
		if _, exists := dir.Commands[k]; !exists {
			if dir.Commands == nil {
				dir.Commands = make(map[string]string)
			}
			dir.Commands[k] = v
		}
	}
	for name, svc := range preset.Services {
		if _, exists := dir.Services[name]; !exists {
			dir.Services[name] = svc
			dir.ServiceOrder = append(dir.ServiceOrder, name)
		}
	}
}
