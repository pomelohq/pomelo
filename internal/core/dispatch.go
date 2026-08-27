package core

import "encoding/json"

// Data-routed verbs (ADR 0001 target shape): one entry point per interaction kind,
// routed by a domain/action string, so a new endpoint is a case here plus a DTO
// instead of a new FFI export. Returns `any` so each domain can hand back its own
// shape (map, struct, slice) and bindingJSON marshals it.

func pStr(params json.RawMessage, key string) string {
	var m map[string]any
	_ = json.Unmarshal(params, &m)
	if v, ok := m[key].(string); ok {
		return v
	}
	return ""
}

func pBool(params json.RawMessage, key string) bool {
	var m map[string]any
	_ = json.Unmarshal(params, &m)
	v, _ := m[key].(bool)
	return v
}

func pInt(params json.RawMessage, key string) int {
	var m map[string]any
	_ = json.Unmarshal(params, &m)
	if v, ok := m[key].(float64); ok {
		return int(v)
	}
	return 0
}

func pStrs(params json.RawMessage, key string) []string {
	var m map[string]any
	_ = json.Unmarshal(params, &m)
	arr, _ := m[key].([]any)
	out := make([]string, 0, len(arr))
	for _, v := range arr {
		if s, ok := v.(string); ok {
			out = append(out, s)
		}
	}
	return out
}

// Query reads a snapshot for a domain. params is the domain's request DTO as JSON.
func (s *Server) Query(domain string, params json.RawMessage) any {
	if s == nil {
		return map[string]any{"error": "no server"}
	}
	switch domain {
	case "workspaces":
		return map[string]any{"workspaces": s.CollectWorkspaces(pBool(params, "git"))}
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
	case "db_list":
		return s.DBList(pStr(params, "branch"))
	case "db_tables":
		return s.DBTables(pStr(params, "branch"), pStr(params, "db"))
	case "db_columns":
		return s.DBColumns(pStr(params, "branch"), pStr(params, "db"))
	case "db_query":
		return s.DBQuery(pStr(params, "branch"), pStr(params, "db"), pStr(params, "sql"), pInt(params, "limit"))
	case "shared_stats":
		return s.SharedStats(pStr(params, "name"))
	case "shared_inspect":
		return s.SharedInspect(pStr(params, "name"))
	case "shared_logs":
		return s.SharedLogs(pStr(params, "name"), pStr(params, "lines"))
	case "config_get":
		return s.ConfigFileGet(pStr(params, "path"))
	case "config_explain":
		return s.ConfigExplain(pStr(params, "repo"), pStr(params, "branch"), pStr(params, "svc"), pStr(params, "env"))
	case "fetch_image":
		return s.FetchImageB64(pStr(params, "url"))
	case "suggest_name":
		return s.SuggestName(pStr(params, "branch"), pStr(params, "desc"))
	case "jira_sprint":
		return s.JiraSprint(pInt(params, "board"))
	case "jira_issue":
		return s.JiraIssue(pStr(params, "key"))
	case "jira_issues":
		return s.JiraIssues(pStrs(params, "branches"))
	case "service_url":
		return s.ServiceURL(pStr(params, "branch"), pStr(params, "repo"), pStr(params, "svc"))
	case "pane_busy":
		return map[string]any{"busy": s.PaneBusy(pStr(params, "holder"))}
	case "claude_terminal":
		return s.ClaudeTerminal(pStr(params, "branch"), pBool(params, "is_main"))
	case "peek_all":
		return map[string]any{"windows": s.PeekWindows(pStrs(params, "windows"), pInt(params, "lines"))}
	case "version":
		return VersionInfo()
	case "secret_get":
		name := pStr(params, "name")
		return map[string]any{"name": name, "value": s.SecretGet(name)}
	case "devproxy_log":
		return s.DevProxyLog(pInt(params, "limit"))
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
	switch domain {
	case "service":
		// action is start|stop|restart; params is the service ref JSON.
		return s.ServiceControlJSON(string(params), action)
	case "config":
		switch action {
		case "reload":
			return s.ConfigReload()
		case "file_set":
			return s.ConfigFileSet(pStr(params, "path"), pStr(params, "yaml"), pBool(params, "dry"))
		case "file_create":
			return s.ConfigFileCreate(pStr(params, "name"), pStr(params, "yaml"))
		}
	case "service_mode":
		return s.ServiceMode(pStr(params, "repo"), pStr(params, "svc"), pStr(params, "mode"))
	case "service_env":
		return s.EnvSet(pStr(params, "branch"), pBool(params, "is_main"), pStr(params, "repo"), pStr(params, "svc"), pStr(params, "env"))
	case "shortcut":
		if action == "run" {
			return s.ShortcutRun(pStr(params, "branch"), pBool(params, "is_main"), pStr(params, "repo"), pStr(params, "cmd"))
		}
	case "editor":
		if action == "open" {
			return s.EditorOpen(pStr(params, "branch"), pBool(params, "is_main"), pStr(params, "repo"), pStr(params, "editor"), pBool(params, "resolve_only"))
		}
	case "github":
		if action == "test" {
			return s.GithubTest(pStr(params, "token"))
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
		case "delete":
			return okErr(s.NMStoreDelete(pStr(params, "repo"), pStr(params, "hash")))
		}
	case "pty":
		if action == "reap" {
			return s.PtyReap()
		}
	case "pane":
		if action == "kill" {
			return s.PaneKill(pStr(params, "pane_id"))
		}
	case "shared":
		return s.SharedAction(pStr(params, "name"), action)
	case "shared_stack":
		return s.SharedStack(action)
	case "session":
		switch action {
		case "switch":
			return s.SessionSwitch(pStr(params, "name"))
		case "delete":
			return s.SessionDelete(pStr(params, "name"), pBool(params, "purge"))
		}
	case "network":
		if action == "set_ports" {
			return s.NetworkSetPorts(pInt(params, "proxy_port"), pInt(params, "webhook_port"))
		}
	case "sync":
		if action == "set" {
			return okErr(s.SyncSet(pBool(params, "refresh_main"), pInt(params, "interval_sec")))
		}
	case "secret":
		if action == "set" {
			return okErr(s.SecretSet(pStr(params, "name"), pStr(params, "value")))
		}
	case "jira":
		switch action {
		case "set":
			return okErr(s.JiraConfigSet(pStr(params, "site"), pStr(params, "email"), pStr(params, "token")))
		case "test":
			return s.JiraTest(pStr(params, "site"), pStr(params, "email"), pStr(params, "token"))
		}
	case "workspace":
		if action == "rename" {
			return s.WorkspaceRename(pStr(params, "branch"), pBool(params, "is_main"), pStr(params, "display_name"))
		}
	case "deps":
		if action == "install" {
			return s.InstallDeps(pStr(params, "branch"), pBool(params, "is_main"))
		}
	case "bundle":
		switch action {
		case "export":
			return s.BundleExport(pBool(params, "include_secrets"), pStr(params, "password"))
		case "read":
			return s.BundleRead(pStr(params, "data_b64"), pStr(params, "password"))
		case "apply":
			return s.BundleApply(pStr(params, "data_b64"), pStr(params, "password"), pStr(params, "yaml"), pBool(params, "write_config"), pBool(params, "create_secrets"))
		case "adapt":
			return s.BundleAdapt(pStr(params, "data_b64"), pStr(params, "password"), pStr(params, "yaml"), pBool(params, "create_secrets"))
		}
	case "db":
		switch action {
		case "export_csv":
			return s.DBExportCSV(pStr(params, "branch"), pStr(params, "db"), pStr(params, "sql"), pStr(params, "path"))
		case "consoles_save":
			return s.DBConsolesSave(pStr(params, "data"))
		}
	case "git":
		switch action {
		case "pull":
			return s.GitPull(pStr(params, "branch"), pStr(params, "repo"), pBool(params, "is_main"))
		case "main_pull":
			return s.MainPull(pStr(params, "branch"))
		}
	}
	return map[string]any{"ok": false, "error": "unknown command: " + domain + "." + action}
}
