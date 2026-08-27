package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"encoding/json"
	"strings"

	"github.com/pomelohq/pomelo/internal/agent/claude"
	"github.com/pomelohq/pomelo/internal/agent/codeagent"
	"github.com/pomelohq/pomelo/internal/core"
)

//export PomClaudeUsage
func PomClaudeUsage() *C.char {
	return bindingJSON(claude.FetchUsage())
}

//export PomCodeAgents
func PomCodeAgents() *C.char {
	type agent struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	}
	out := []agent{}
	for _, ca := range codeagent.Builtin() {
		out = append(out, agent{ID: ca.Cmd, Name: ca.Name})
	}
	return bindingJSON(out)
}

func server() *core.Server {
	mu.Lock()
	defer mu.Unlock()
	return srv
}

func bindingJSON(v any) *C.char {
	b, err := json.Marshal(v)
	if err != nil {
		return C.CString("{}")
	}
	return C.CString(string(b))
}

func bindingBytes(b []byte) *C.char { return C.CString(string(b)) }

//export PomWorkspaces
func PomWorkspaces(git C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"workspaces":[]}`)
	}
	return bindingJSON(map[string]any{"workspaces": s.CollectWorkspaces(git != 0)})
}

//export PomLiveness
func PomLiveness() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"workspaces":[]}`)
	}
	return bindingJSON(map[string]any{"workspaces": s.CollectLiveness()})
}

//export PomDoctor
func PomDoctor() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"findings":[],"errors":0,"warnings":0}`)
	}
	return bindingJSON(s.DoctorReport())
}

//export PomConfigFiles
func PomConfigFiles() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"files":[]}`)
	}
	return bindingJSON(s.ConfigRead())
}

//export PomNMStoreList
func PomNMStoreList() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"entries":[],"total":0}`)
	}
	return bindingJSON(s.NMStoreList())
}

//export PomNMStoreReconcile
func PomNMStoreReconcile() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.NMStoreReconcile())
}

//export PomNMStoreReclaim
func PomNMStoreReclaim() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.NMStoreReclaim())
}

//export PomNMStoreProgress
func PomNMStoreProgress() *C.char {
	s := server()
	if s == nil {
		return C.CString("")
	}
	return C.CString(s.NMStoreProgress())
}

//export PomNMStoreDelete
func PomNMStoreDelete(repo, hash *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	if err := s.NMStoreDelete(C.GoString(repo), C.GoString(hash)); err != nil {
		return bindingJSON(map[string]any{"ok": false, "error": err.Error()})
	}
	return C.CString(`{"ok":true}`)
}

//export PomSharedStatus
func PomSharedStatus() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"running":{},"urls":{},"services":[]}`)
	}
	return bindingJSON(s.SharedStatus())
}

//export PomSharedStats
func PomSharedStats(name *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"running":false}`)
	}
	return bindingJSON(s.SharedStats(C.GoString(name)))
}

//export PomDBList
func PomDBList(branch *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.DBList(C.GoString(branch)))
}

//export PomDBTables
func PomDBTables(branch, db *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.DBTables(C.GoString(branch), C.GoString(db)))
}

//export PomDBExportCSV
func PomDBExportCSV(branch, db, sql, path *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.DBExportCSV(C.GoString(branch), C.GoString(db), C.GoString(sql), C.GoString(path)))
}

//export PomDBColumns
func PomDBColumns(branch, db *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.DBColumns(C.GoString(branch), C.GoString(db)))
}

//export PomDBQuery
func PomDBQuery(branch, db, sql *C.char, limit C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.DBQuery(C.GoString(branch), C.GoString(db), C.GoString(sql), int(limit)))
}

//export PomDBConsolesLoad
func PomDBConsolesLoad() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":true,"consoles":[]}`)
	}
	return bindingJSON(s.DBConsolesLoad())
}

//export PomDBConsolesSave
func PomDBConsolesSave(data *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.DBConsolesSave(C.GoString(data)))
}

//export PomMCPStatus
func PomMCPStatus() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"registered":false,"connected":false}`)
	}
	return bindingJSON(s.MCPGlobalStatus())
}

//export PomMCPReinstall
func PomMCPReinstall() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.MCPGlobalReinstall())
}

//export PomFetchImage
func PomFetchImage(rawURL *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false,"error":"no server"}`)
	}
	return bindingJSON(s.FetchImageB64(C.GoString(rawURL)))
}

//export PomSharedInspect
func PomSharedInspect(name *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString("{}")
	}
	return bindingJSON(s.SharedInspect(C.GoString(name)))
}

//export PomSharedLogs
func PomSharedLogs(name, lines *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"running":false,"lines":[]}`)
	}
	return bindingJSON(s.SharedLogs(C.GoString(name), C.GoString(lines)))
}

//export PomOpenEditor
func PomOpenEditor(branch *C.char, isMain C.int, repo, editor *C.char, resolveOnly C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.EditorOpen(C.GoString(branch), isMain != 0, C.GoString(repo), C.GoString(editor), resolveOnly != 0))
}

//export PomClaudeTerminal
func PomClaudeTerminal(branch *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"error":"not initialized"}`)
	}
	return bindingJSON(s.ClaudeTerminal(C.GoString(branch), isMain != 0))
}

//export PomBundleExport
func PomBundleExport(includeSecrets C.int, password *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString("{}")
	}
	return bindingJSON(s.BundleExport(includeSecrets != 0, C.GoString(password)))
}

//export PomBundleRead
func PomBundleRead(dataB64, password *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString("{}")
	}
	return bindingJSON(s.BundleRead(C.GoString(dataB64), C.GoString(password)))
}

//export PomBundleApply
func PomBundleApply(dataB64, password, yaml *C.char, writeConfig, createSecrets C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.BundleApply(C.GoString(dataB64), C.GoString(password), C.GoString(yaml), writeConfig != 0, createSecrets != 0))
}

//export PomBundleAdapt
func PomBundleAdapt(dataB64, password, yaml *C.char, createSecrets C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.BundleAdapt(C.GoString(dataB64), C.GoString(password), C.GoString(yaml), createSecrets != 0))
}

//export PomConfigExplain
func PomConfigExplain(repo, branch, svc, env *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"repos":[]}`)
	}
	return bindingJSON(s.ConfigExplain(C.GoString(repo), C.GoString(branch), C.GoString(svc), C.GoString(env)))
}

//export PomEnvironments
func PomEnvironments() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"environments":[]}`)
	}
	return bindingJSON(s.Environments())
}

//export PomSuggestName
func PomSuggestName(branch, desc *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"name":"","slug":""}`)
	}
	return bindingJSON(s.SuggestName(C.GoString(branch), C.GoString(desc)))
}

//export PomShortcutRun
func PomShortcutRun(branch *C.char, isMain C.int, repo, cmd *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.ShortcutRun(C.GoString(branch), isMain != 0, C.GoString(repo), C.GoString(cmd)))
}

//export PomEnvSet
func PomEnvSet(branch *C.char, isMain C.int, repo, svc, env *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.EnvSet(C.GoString(branch), isMain != 0, C.GoString(repo), C.GoString(svc), C.GoString(env)))
}

//export PomServiceMode
func PomServiceMode(repo, svc, mode *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.ServiceMode(C.GoString(repo), C.GoString(svc), C.GoString(mode)))
}

//export PomServiceURL
func PomServiceURL(branch, repo, svc *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString("{}")
	}
	return bindingJSON(s.ServiceURL(C.GoString(branch), C.GoString(repo), C.GoString(svc)))
}

//export PomServiceControl
func PomServiceControl(refJSON, action *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.ServiceControlJSON(C.GoString(refJSON), C.GoString(action)))
}

//export PomPtyReap
func PomPtyReap() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"reaped":0}`)
	}
	return bindingJSON(s.PtyReap())
}

//export PomPaneKill
func PomPaneKill(paneID *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.PaneKill(C.GoString(paneID)))
}

//export PomWorkspaceRename
func PomWorkspaceRename(branch *C.char, isMain C.int, displayName *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.WorkspaceRename(C.GoString(branch), isMain != 0, C.GoString(displayName)))
}

//export PomEditors
func PomEditors() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"installed":[],"configured":""}`)
	}
	return bindingJSON(s.Editors())
}

//export PomNetworkSetPorts
func PomNetworkSetPorts(proxyPort, webhookPort C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.NetworkSetPorts(int(proxyPort), int(webhookPort)))
}

//export PomNetwork
func PomNetwork() *C.char {
	s := server()
	if s == nil {
		return C.CString("{}")
	}
	return bindingJSON(s.NetworkInfo())
}

//export PomDevProxyLog
func PomDevProxyLog(limit C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"entries":[]}`)
	}
	return bindingJSON(s.DevProxyLog(int(limit)))
}

//export PomPaneBusy
func PomPaneBusy(holder *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"busy":false}`)
	}
	return bindingJSON(map[string]any{"busy": s.PaneBusy(C.GoString(holder))})
}

//export PomSessionList
func PomSessionList() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"sessions":[]}`)
	}
	return bindingJSON(s.SessionList())
}

//export PomSessionSwitch
func PomSessionSwitch(name *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.SessionSwitch(C.GoString(name)))
}

//export PomSessionDelete
func PomSessionDelete(name *C.char, purge C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.SessionDelete(C.GoString(name), purge != 0))
}

//export PomSessionCreate
func PomSessionCreate(reqJSON *C.char) *C.char {
	var req core.CreateSessionReq
	if err := json.Unmarshal([]byte(C.GoString(reqJSON)), &req); err != nil {
		return bindingJSON(map[string]any{"ok": false, "error": "bad json"})
	}
	dir, err := core.ScaffoldSession(req)
	if err != nil {
		return bindingJSON(map[string]any{"ok": false, "error": err.Error()})
	}
	return bindingJSON(map[string]any{"ok": true, "name": req.Name, "path": dir})
}

//export PomConfigFileGet
func PomConfigFileGet(path *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString("{}")
	}
	return bindingJSON(s.ConfigFileGet(C.GoString(path)))
}

//export PomConfigFileSet
func PomConfigFileSet(path, yaml *C.char, dry C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.ConfigFileSet(C.GoString(path), C.GoString(yaml), dry != 0))
}

//export PomConfigFileCreate
func PomConfigFileCreate(name, yaml *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.ConfigFileCreate(C.GoString(name), C.GoString(yaml)))
}

//export PomInstallDeps
func PomInstallDeps(branch *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.InstallDeps(C.GoString(branch), isMain != 0))
}

//export PomConfigReload
func PomConfigReload() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"error":"not initialized"}`)
	}
	return bindingJSON(s.ConfigReload())
}

//export PomSharedAction
func PomSharedAction(name, action *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.SharedAction(C.GoString(name), C.GoString(action)))
}

//export PomSharedStack
func PomSharedStack(action *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.SharedStack(C.GoString(action)))
}

//export PomIntegrationsStatus
func PomIntegrationsStatus() *C.char {
	s := server()
	if s == nil {
		return C.CString("{}")
	}
	return bindingJSON(s.IntegrationsStatus())
}

//export PomSecretNames
func PomSecretNames() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"names":[]}`)
	}
	return bindingJSON(map[string]any{"names": s.SecretNames()})
}

//export PomSecretGet
func PomSecretGet(name *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"value":""}`)
	}
	return bindingJSON(map[string]any{"name": C.GoString(name), "value": s.SecretGet(C.GoString(name))})
}

//export PomSecretSet
func PomSecretSet(name, value *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	if err := s.SecretSet(C.GoString(name), C.GoString(value)); err != nil {
		return bindingJSON(map[string]any{"ok": false, "error": err.Error()})
	}
	return C.CString(`{"ok":true}`)
}

//export PomJiraSet
func PomJiraSet(site, email, token *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	if err := s.JiraConfigSet(C.GoString(site), C.GoString(email), C.GoString(token)); err != nil {
		return bindingJSON(map[string]any{"ok": false, "error": err.Error()})
	}
	return C.CString(`{"ok":true}`)
}

//export PomVersion
func PomVersion() *C.char {
	return bindingJSON(core.VersionInfo())
}

//export PomPs
func PomPs() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"processes":[],"total":{"cpu":0,"ram_mb":0,"procs":0}}`)
	}
	return bindingJSON(s.PsData())
}

//export PomMainPull
func PomMainPull(branch *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.MainPull(C.GoString(branch)))
}

//export PomGitPull
func PomGitPull(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.GitPull(C.GoString(branch), C.GoString(repo), isMain != 0))
}

//export PomPRAll
func PomPRAll() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{}`)
	}
	return bindingBytes(s.PRAllPRs())
}

//export PomPRWorkspace
func PomPRWorkspace(branch *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"prs":[]}`)
	}
	return bindingBytes(s.PRWorkspacePRs(C.GoString(branch), isMain != 0))
}

//export PomPRDetail
func PomPRDetail(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"pr":null}`)
	}
	return bindingBytes(s.PRDetail(C.GoString(branch), C.GoString(repo), isMain != 0))
}

//export PomPRComments
func PomPRComments(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"comments":[]}`)
	}
	return bindingBytes(s.PRComments(C.GoString(branch), C.GoString(repo), isMain != 0))
}

//export PomPRCommits
func PomPRCommits(branch, repo, base *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"commits":[]}`)
	}
	return bindingJSON(s.PRCommits(C.GoString(branch), C.GoString(repo), C.GoString(base), isMain != 0))
}

//export PomPRDiff
func PomPRDiff(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString("")
	}
	out, err := s.PRDiff(C.GoString(branch), C.GoString(repo), isMain != 0)
	if err != nil {
		return C.CString("")
	}
	return bindingBytes(out)
}

//export PomWorkspaceLocalChanges
func PomWorkspaceLocalChanges(branch *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"repos":[]}`)
	}
	return bindingBytes(s.WorkspaceLocalChanges(C.GoString(branch), isMain != 0))
}

//export PomLocalDiff
func PomLocalDiff(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString("")
	}
	out, err := s.LocalDiff(C.GoString(branch), C.GoString(repo), isMain != 0)
	if err != nil {
		return C.CString("")
	}
	return bindingBytes(out)
}

//export PomGithubTest
func PomGithubTest(token *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.GithubTest(C.GoString(token)))
}

//export PomJiraTest
func PomJiraTest(site, email, token *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	return bindingJSON(s.JiraTest(C.GoString(site), C.GoString(email), C.GoString(token)))
}

//export PomJiraBoards
func PomJiraBoards() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"configured":false}`)
	}
	return bindingJSON(s.JiraBoards())
}

//export PomJiraSprint
func PomJiraSprint(board C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"configured":false}`)
	}
	return bindingJSON(s.JiraSprint(int(board)))
}

//export PomJiraIssue
func PomJiraIssue(key *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"configured":false}`)
	}
	return bindingJSON(s.JiraIssue(C.GoString(key)))
}

//export PomJiraIssues
func PomJiraIssues(branchesCSV *C.char) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"configured":false}`)
	}
	var branches []string
	if csv := C.GoString(branchesCSV); csv != "" {
		branches = strings.Split(csv, ",")
	}
	return bindingJSON(s.JiraIssues(branches))
}

//export PomSyncGet
func PomSyncGet() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"refresh_main":false,"refresh_interval_sec":1800}`)
	}
	return bindingJSON(s.SyncGet())
}

//export PomSyncSet
func PomSyncSet(refreshMain C.int, intervalSec C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"ok":false}`)
	}
	if err := s.SyncSet(refreshMain != 0, int(intervalSec)); err != nil {
		return bindingJSON(map[string]any{"ok": false, "error": err.Error()})
	}
	return C.CString(`{"ok":true}`)
}

//export PomLogs
func PomLogs() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"lines":[]}`)
	}
	return bindingJSON(s.LogsData())
}

//export PomPeekAll
func PomPeekAll(windows *C.char, lines C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"windows":{}}`)
	}
	var w []string
	if csv := C.GoString(windows); csv != "" {
		w = strings.Split(csv, ",")
	}
	return bindingJSON(map[string]any{"windows": s.PeekWindows(w, int(lines))})
}
