package config

import (
	"encoding/json"
	"os"
	"testing"
)

// TestOracleDump writes a stable JSON projection of Load for the Rust port's parity tests.
func TestOracleDump(t *testing.T) {
	path, out := os.Getenv("POM_ORACLE_CONFIG"), os.Getenv("POM_ORACLE_OUT")
	if path == "" || out == "" {
		t.Skip("oracle dump only runs when driven by the parity tests")
	}
	result := map[string]any{}
	cfg, err := Load(path)
	if err != nil {
		result["load_error"] = true
	} else {
		result = oracleProject(cfg)
		result["valid"] = cfg.Validate() == nil
	}
	data, err := json.Marshal(result)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(out, data, 0o644); err != nil {
		t.Fatal(err)
	}
}

func oracleProject(c *Config) map[string]any {
	var repos []any
	for _, name := range c.RepoOrder {
		d := c.Repos[name]
		if d == nil {
			continue
		}
		var services []any
		for _, sn := range d.ServiceOrder {
			s := d.Services[sn]
			if s == nil {
				continue
			}
			services = append(services, map[string]any{
				"name": sn, "type": s.Type, "cmd": s.Cmd, "dir": s.Dir, "shell_env": s.ShellEnv,
				"env": s.Env, "pre_start": s.PreStart, "proxy_port": s.ProxyPort,
				"shortcuts": oracleShortcuts(s.Shortcuts), "depends_on": s.DependsOn,
				"has_port": s.HasPort(), "modes": s.Modes, "mode": s.Mode, "profiles": []string(s.Profiles),
			})
		}
		var envFiles []any
		for _, e := range d.EnvFileEntries() {
			envFiles = append(envFiles, map[string]any{"file": e.File, "env": e.Env})
		}
		var refs []any
		for _, r := range d.SharedSvcRefs {
			refs = append(refs, map[string]any{"name": r.Name, "db_name": r.DBName})
		}
		repos = append(repos, map[string]any{
			"name": name, "alias": d.Alias, "pre_start": d.PreStart, "shell_env": d.ShellEnv,
			"default_branch": d.DefaultBranch, "services": services, "proxy_port": d.ProxyPort,
			"profiles": []string(d.Profiles), "copy": d.Copy, "env": d.Env, "own_env": d.OwnEnv(),
			"env_files": envFiles, "shared_refs": refs, "databases": d.Databases, "presets": d.Presets_,
			"setup": d.Setup, "migrate": d.Migrate, "seed": d.Seed, "seed_from_main": d.SeedFromMain,
			"commands": d.Commands, "pre_delete": d.PreDelete,
			"effective_setup": d.EffectiveSetup(), "effective_migrate": d.EffectiveMigrate(),
			"effective_shortcuts": oracleShortcuts(d.EffectiveShortcuts()),
			"has_worktree_config": d.HasWorktreeConfig(),
		})
	}
	shared := map[string]any{}
	for name, s := range c.SharedServices {
		entry := map[string]any{
			"type": s.Type, "image": s.Image, "host": s.Host, "ports": s.Ports,
			"environment": s.Environment, "volumes": s.Volumes, "command": s.Command,
			"db_user": s.DBUser, "db_password": s.DBPassword, "capacity": s.Capacity,
		}
		if s.Healthcheck != nil {
			entry["healthcheck"] = map[string]any{
				"interval": s.Healthcheck.Interval, "timeout": s.Healthcheck.Timeout,
				"retries": s.Healthcheck.Retries,
			}
		}
		shared[name] = entry
	}
	var wsServices []any
	for _, name := range c.WsServiceOrder {
		wsServices = append(wsServices, map[string]any{"name": name, "cmd": c.WsServices[name].Cmd})
	}
	return map[string]any{
		"session": c.Session, "default_branch": c.GlobalDefaultBranch(), "repos": repos,
		"shared_services": shared, "workspaces": c.AllWorkspaces(), "environments": c.Environments,
		"seed": c.Seed, "prepare_main": c.PrepareMainPhases(), "workspace_services": wsServices,
	}
}

func oracleShortcuts(in []Shortcut) []any {
	var out []any
	for _, s := range in {
		out = append(out, map[string]any{"cmd": s.Cmd, "desc": s.Desc, "key": s.Key})
	}
	return out
}
