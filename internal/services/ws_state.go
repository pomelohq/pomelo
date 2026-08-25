package services

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
)

type WorkspaceState struct {
	ServiceEnvs map[string]string `json:"service_envs,omitempty"`
	DisplayName string            `json:"display_name,omitempty"`
}

func wsStatePath(wsFolder string) string {
	return filepath.Join(wsFolder, ".pom-workspace.json")
}

func LoadWorkspaceState(wsFolder string) WorkspaceState {
	data, err := os.ReadFile(wsStatePath(wsFolder))
	if err != nil {
		return WorkspaceState{}
	}
	var state WorkspaceState
	_ = json.Unmarshal(data, &state)
	return state
}

func SaveWorkspaceState(wsFolder string, state *WorkspaceState) {
	data, _ := json.MarshalIndent(state, "", "  ")
	_ = os.WriteFile(wsStatePath(wsFolder), append(data, '\n'), 0o644)
}

func SetWorkspaceDisplayName(wsFolder, name string) error {
	state := LoadWorkspaceState(wsFolder)
	state.DisplayName = name
	data, err := json.MarshalIndent(&state, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(wsStatePath(wsFolder), append(data, '\n'), 0o644)
}

func ServiceEnvironment(configDir, branch, svcKey string) string {
	wsFolder := filepath.Join(configDir, "workspace--"+branch)
	state := LoadWorkspaceState(wsFolder)
	if state.ServiceEnvs == nil {
		return ""
	}
	if env, ok := state.ServiceEnvs[svcKey]; ok {
		return env
	}
	if idx := strings.Index(svcKey, "/"); idx > 0 {
		return state.ServiceEnvs[svcKey[:idx]]
	}
	return ""
}
