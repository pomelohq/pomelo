package core

import "encoding/json"

// Data-routed verbs (ADR 0001 target shape): one entry point per interaction kind,
// routed by a domain/action string, so a new endpoint is a case here plus a DTO
// instead of a new FFI export. Returns `any` so each domain can hand back its own
// shape (map, struct, slice) and bindingJSON marshals it.

// Query reads a snapshot for a domain. params is the domain's request DTO as JSON.
func (s *Server) Query(domain string, params json.RawMessage) any {
	if s == nil {
		return map[string]any{"error": "no server"}
	}
	switch domain {
	case "workspaces":
		var p struct {
			Git bool `json:"git"`
		}
		_ = json.Unmarshal(params, &p)
		return map[string]any{"workspaces": s.CollectWorkspaces(p.Git)}
	case "liveness":
		return map[string]any{"workspaces": s.CollectLiveness()}
	case "agent_states":
		return map[string]any{"states": s.AgentStates()}
	case "doctor":
		return s.DoctorReport()
	case "config_files":
		return s.ConfigRead()
	case "nmstore_list":
		return s.NMStoreList()
	case "shared_status":
		return s.SharedStatus()
	case "mcp_status":
		return s.MCPGlobalStatus()
	case "integrations_status":
		return s.IntegrationsStatus()
	case "environments":
		return s.Environments()
	case "editors":
		return s.Editors()
	case "network":
		return s.NetworkInfo()
	case "session_list":
		return s.SessionList()
	case "sync_get":
		return s.SyncGet()
	case "logs":
		return s.LogsData()
	case "ps":
		return s.PsData()
	case "db_consoles_load":
		return s.DBConsolesLoad()
	case "jira_boards":
		return s.JiraBoards()
	case "secret_names":
		return map[string]any{"names": s.SecretNames()}
	default:
		return map[string]any{"error": "unknown query domain: " + domain}
	}
}

// Command performs a mutation for a domain. params is the action's request DTO as JSON.
func (s *Server) Command(domain, action string, params json.RawMessage) any {
	if s == nil {
		return map[string]any{"ok": false, "error": "no server"}
	}
	okErr := func(err error) map[string]any {
		if err != nil {
			return map[string]any{"ok": false, "error": err.Error()}
		}
		return map[string]any{"ok": true}
	}
	arg := func(key string) string {
		var m map[string]any
		_ = json.Unmarshal(params, &m)
		if v, ok := m[key].(string); ok {
			return v
		}
		return ""
	}
	flag := func(key string) bool {
		var m map[string]any
		_ = json.Unmarshal(params, &m)
		v, _ := m[key].(bool)
		return v
	}
	num := func(key string) int {
		var m map[string]any
		_ = json.Unmarshal(params, &m)
		if v, ok := m[key].(float64); ok {
			return int(v)
		}
		return 0
	}
	switch domain {
	case "service":
		// action is start|stop|restart; params is the service ref JSON.
		return s.ServiceControlJSON(string(params), action)
	case "config":
		if action == "reload" {
			return s.ConfigReload()
		}
	case "mcp":
		if action == "reinstall" {
			return s.MCPGlobalReinstall()
		}
	case "nmstore":
		switch action {
		case "reconcile":
			return s.NMStoreReconcile()
		case "reclaim":
			return s.NMStoreReclaim()
		}
	case "pty":
		if action == "reap" {
			return s.PtyReap()
		}
	case "pane":
		if action == "kill" {
			return s.PaneKill(arg("pane_id"))
		}
	case "shared":
		return s.SharedAction(arg("name"), action)
	case "shared_stack":
		return s.SharedStack(action)
	case "session":
		switch action {
		case "switch":
			return s.SessionSwitch(arg("name"))
		case "delete":
			return s.SessionDelete(arg("name"), flag("purge"))
		}
	case "network":
		if action == "set_ports" {
			return s.NetworkSetPorts(num("proxy_port"), num("webhook_port"))
		}
	case "sync":
		if action == "set" {
			return okErr(s.SyncSet(flag("refresh_main"), num("interval_sec")))
		}
	case "secret":
		if action == "set" {
			return okErr(s.SecretSet(arg("name"), arg("value")))
		}
	case "jira":
		if action == "set" {
			return okErr(s.JiraConfigSet(arg("site"), arg("email"), arg("token")))
		}
	case "workspace":
		if action == "rename" {
			return s.WorkspaceRename(arg("branch"), flag("is_main"), arg("display_name"))
		}
	case "deps":
		if action == "install" {
			return s.InstallDeps(arg("branch"), flag("is_main"))
		}
	}
	return map[string]any{"ok": false, "error": "unknown command: " + domain + "." + action}
}
