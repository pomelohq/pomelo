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
	switch domain {
	case "service":
		// action is start|stop|restart; params is the service ref JSON.
		return s.ServiceControlJSON(string(params), action)
	default:
		return map[string]any{"ok": false, "error": "unknown command domain: " + domain}
	}
}
