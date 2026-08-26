import Foundation
@testable import PomeloApp

final class MockPomAPI: PomAPI {
    var doctorJSON = #"{"findings":[],"errors":0,"warnings":0}"#
    var workspacesJSON = #"{"workspaces":[]}"#
    var agentStatesJSON = #"{"states":{}}"#
    var peekAllJSON = #"{"windows":{}}"#
    var configFilesJSON = #"{"files":[]}"#
    var logsJSON = #"{"lines":[]}"#
    var nmStoreJSON = #"{"entries":[],"total":0}"#
    private(set) var nmDeleted: [String] = []
    var syncJSON = #"{"refresh_main":false,"refresh_interval_sec":1800}"#
    private(set) var syncSetCalls: [(Bool, Int)] = []

    func doctorData() -> Data { Data(doctorJSON.utf8) }
    func logsData() -> Data { Data(logsJSON.utf8) }
    func configFileGetData(path: String) -> Data { Data("{}".utf8) }
    func configFileSet(path: String, yaml: String, dry: Bool) -> Data { Data(#"{"ok":true}"#.utf8) }
    func configFileCreate(name: String, yaml: String) -> Data { Data(#"{"ok":true,"path":"pom.d/\#(name)","name":"\#(name)"}"#.utf8) }
    func installDeps(branch: String, isMain: Bool) -> Data { Data(#"{"ok":true,"failed":[]}"#.utf8) }
    func configReload() -> Data { Data(#"{"ok":true}"#.utf8) }
    func nmStoreListData() -> Data { Data(nmStoreJSON.utf8) }
    func nmStoreDelete(repo: String, hash: String) -> Data { nmDeleted.append(repo + "/" + hash); return Data(#"{"ok":true}"#.utf8) }
    func nmStoreReconcile() -> Data { Data(#"{"ok":true,"added":0,"bytes":0}"#.utf8) }
    func nmStoreReclaim() -> Data { Data(#"{"ok":true,"relinked":0,"reclaimed":0}"#.utf8) }
    func nmStoreProgress() -> String { "" }
    func codeAgentsData() -> Data { Data(#"[{"id":"claude","name":"Claude Code"}]"#.utf8) }
    func syncGetData() -> Data { Data(syncJSON.utf8) }
    func syncSet(refreshMain: Bool, intervalSec: Int) -> Data { syncSetCalls.append((refreshMain, intervalSec)); return Data(#"{"ok":true}"#.utf8) }

    var integrationsJSON = "{}"
    var secretNamesJSON = #"{"names":[]}"#
    var secretValues: [String: String] = [:]
    private(set) var secretSetCalls: [(String, String)] = []
    var sharedStatusJSON = #"{"running":{},"urls":{},"services":[]}"#
    func sharedStatusData() -> Data { Data(sharedStatusJSON.utf8) }
    func mcpStatusData() -> Data { Data(#"{"registered":true,"connected":true,"command":"sh …/pom-mcp"}"#.utf8) }
    func mcpReinstallData() -> Data { Data(#"{"ok":true}"#.utf8) }
    func fetchImageData(url: String) -> Data { Data(#"{"ok":false,"error":"mock"}"#.utf8) }
    func sharedStatsData(name: String) -> Data { Data(#"{"running":false}"#.utf8) }
    func sharedInspectData(name: String) -> Data { Data("{}".utf8) }
    func dbListData(branch: String) -> Data { Data(#"{"ok":true,"databases":[]}"#.utf8) }
    func dbTablesData(branch: String, db: String) -> Data { Data(#"{"ok":true,"tables":[]}"#.utf8) }
    func dbQueryData(branch: String, db: String, sql: String, limit: Int) -> Data { Data(#"{"ok":true,"columns":[],"rows":[]}"#.utf8) }
    func dbColumnsData(branch: String, db: String) -> Data { Data(#"{"ok":true,"columns":[]}"#.utf8) }
    func dbExportCSV(branch: String, db: String, sql: String, path: String) -> Data { Data(#"{"ok":true,"rows":0}"#.utf8) }
    func dbConsolesLoadData() -> Data { Data(#"{"ok":true,"consoles":[]}"#.utf8) }
    func dbConsolesSave(json: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func sharedLogsData(name: String, lines: Int) -> Data { Data(#"{"running":false,"lines":[]}"#.utf8) }
    func sharedAction(name: String, action: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func sharedStack(action: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func integrationsStatusData() -> Data { Data(integrationsJSON.utf8) }
    func secretNamesData() -> Data { Data(secretNamesJSON.utf8) }
    func secretGet(name: String) -> Data { Data("{\"name\":\"\(name)\",\"value\":\"\(secretValues[name] ?? "")\"}".utf8) }
    func secretSet(name: String, value: String) -> Data { secretSetCalls.append((name, value)); return Data(#"{"ok":true}"#.utf8) }
    func jiraSet(site: String, email: String, token: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func prAllData() -> Data { Data("{}".utf8) }
    func prWorkspaceData(branch: String, isMain: Bool) -> Data { Data(#"{"prs":[]}"#.utf8) }
    func prDetailData(branch: String, repo: String, isMain: Bool) -> Data { Data(#"{"pr":null}"#.utf8) }
    func prCommentsData(branch: String, repo: String, isMain: Bool) -> Data { Data(#"{"comments":[]}"#.utf8) }
    func prCommitsData(branch: String, repo: String, base: String, isMain: Bool) -> Data { Data(#"{"commits":[]}"#.utf8) }
    func prDiffData(branch: String, repo: String, isMain: Bool) -> Data { Data() }
    func jiraBoardsData() -> Data { Data(#"{"configured":false}"#.utf8) }
    func jiraSprintData(board: Int) -> Data { Data(#"{"configured":false}"#.utf8) }
    func jiraIssueData(key: String) -> Data { Data(#"{"configured":false}"#.utf8) }
    func jiraIssuesData(branches: [String]) -> Data { Data(#"{"configured":false}"#.utf8) }
    func workspacesData(git: Bool) -> Data { Data(workspacesJSON.utf8) }
    func livenessData() -> Data { Data(workspacesJSON.utf8) }
    func agentStatesData() -> Data { Data(agentStatesJSON.utf8) }
    func peekAllData(windows: [String], lines: Int) -> Data { Data(peekAllJSON.utf8) }
    func configFilesData() -> Data { Data(configFilesJSON.utf8) }
    func openEditor(branch: String, isMain: Bool, repo: String, editor: String, resolveOnly: Bool) -> Data { Data(#"{"ok":true}"#.utf8) }
    func claudeTerminal(branch: String, isMain: Bool) -> Data { Data(#"{"pane_id":"","window":""}"#.utf8) }
    func bundleExport(includeSecrets: Bool, password: String) -> Data { Data(#"{"filename":"x","data":""}"#.utf8) }
    func bundleRead(dataB64: String, password: String) -> Data { Data(#"{"yaml":"","secret_names":[]}"#.utf8) }
    func bundleApply(dataB64: String, password: String, yaml: String, writeConfig: Bool, createSecrets: Bool) -> Data { Data(#"{"ok":true}"#.utf8) }
    func bundleAdapt(dataB64: String, password: String, yaml: String, createSecrets: Bool) -> Data { Data(#"{"ok":true}"#.utf8) }
    func configExplainData(repo: String, branch: String, svc: String, env: String) -> Data { Data(#"{"repos":[]}"#.utf8) }
    func environmentsData() -> Data { Data(#"{"environments":[]}"#.utf8) }
    func suggestName(branch: String, desc: String) -> Data { Data(#"{"name":"","slug":""}"#.utf8) }
    func serviceControl(refJSON: String, action: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func shortcutRun(branch: String, isMain: Bool, repo: String, cmd: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func envSet(branch: String, isMain: Bool, repo: String, svc: String, env: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func serviceMode(repo: String, svc: String, mode: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func serviceURL(branch: String, repo: String, svc: String) -> Data { Data(#"{"url":""}"#.utf8) }
    func ptyReap() -> Data { Data(#"{"reaped":0}"#.utf8) }
    func paneKill(paneID: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func workspaceRename(branch: String, isMain: Bool, displayName: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func editorsData() -> Data { Data(#"{"installed":[],"configured":""}"#.utf8) }
    func networkData() -> Data { Data("{}".utf8) }
    func networkSetPorts(proxyPort: Int, webhookPort: Int) -> Data { Data(#"{"ok":true}"#.utf8) }
    func devProxyLogData(limit: Int) -> Data { Data(#"{"entries":[]}"#.utf8) }
    func paneBusyData(holder: String) -> Data { Data(#"{"busy":false}"#.utf8) }
    var sessionListJSON = #"{"sessions":[]}"#
    func sessionListData() -> Data { Data(sessionListJSON.utf8) }
    func sessionSwitch(name: String) -> Data { Data(#"{"ok":true}"#.utf8) }
    func sessionDelete(name: String, purge: Bool) -> Data { Data(#"{"ok":true}"#.utf8) }
    func sessionCreate(json: String) -> Data { Data(#"{"ok":true}"#.utf8) }

}
