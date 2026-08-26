import Foundation


protocol PomBaseAPI: AnyObject {}

extension PomBaseAPI {
    func call(_ work: @escaping (Self) -> Data) async -> Data {
        await Task.detached(priority: .userInitiated) { [self] in work(self) }.value
    }
}

protocol WorkspaceAPI: PomBaseAPI {
    func workspacesData(git: Bool) -> Data
    func livenessData() -> Data
    func agentStatesData() -> Data
    func peekAllData(windows: [String], lines: Int) -> Data
    func workspaceRename(branch: String, isMain: Bool, displayName: String) -> Data
    func suggestName(branch: String, desc: String) -> Data
}

protocol TerminalAPI: PomBaseAPI {
    func claudeTerminal(branch: String, isMain: Bool) -> Data
    func ptyReap() -> Data
    func paneKill(paneID: String) -> Data
    func paneBusyData(holder: String) -> Data
}

protocol ServiceAPI: PomBaseAPI {
    func serviceControl(refJSON: String, action: String) -> Data
    func shortcutRun(branch: String, isMain: Bool, repo: String, cmd: String) -> Data
    func envSet(branch: String, isMain: Bool, repo: String, svc: String, env: String) -> Data
    func serviceMode(repo: String, svc: String, mode: String) -> Data
    func serviceURL(branch: String, repo: String, svc: String) -> Data
    func openEditor(branch: String, isMain: Bool, repo: String, editor: String, resolveOnly: Bool) -> Data
    func editorsData() -> Data
}

protocol ConfigAPI: PomBaseAPI {
    func doctorData() -> Data
    func configFilesData() -> Data
    func configFileGetData(path: String) -> Data
    func configFileSet(path: String, yaml: String, dry: Bool) -> Data
    func configFileCreate(name: String, yaml: String) -> Data
    func installDeps(branch: String, isMain: Bool) -> Data
    func configReload() -> Data
    func configExplainData(repo: String, branch: String, svc: String, env: String) -> Data
    func environmentsData() -> Data
    func networkData() -> Data
    func networkSetPorts(proxyPort: Int, webhookPort: Int) -> Data
    func devProxyLogData(limit: Int) -> Data
}

protocol DBAPI: PomBaseAPI {
    func dbListData(branch: String) -> Data
    func dbTablesData(branch: String, db: String) -> Data
    func dbQueryData(branch: String, db: String, sql: String, limit: Int) -> Data
    func dbColumnsData(branch: String, db: String) -> Data
    func dbExportCSV(branch: String, db: String, sql: String, path: String) -> Data
    func dbConsolesLoadData() -> Data
    func dbConsolesSave(json: String) -> Data
}

protocol SharedServiceAPI: PomBaseAPI {
    func sharedStatusData() -> Data
    func sharedStatsData(name: String) -> Data
    func sharedInspectData(name: String) -> Data
    func sharedLogsData(name: String, lines: Int) -> Data
    func sharedAction(name: String, action: String) -> Data
    func sharedStack(action: String) -> Data
}

protocol SecretsAPI: PomBaseAPI {
    func secretNamesData() -> Data
    func secretGet(name: String) -> Data
    func secretSet(name: String, value: String) -> Data
    func integrationsStatusData() -> Data
    func jiraSet(site: String, email: String, token: String) -> Data
    func mcpStatusData() -> Data
    func mcpReinstallData() -> Data
}

protocol PRAPI: PomBaseAPI {
    func prAllData() -> Data
    func prWorkspaceData(branch: String, isMain: Bool) -> Data
    func prDetailData(branch: String, repo: String, isMain: Bool) -> Data
    func prCommentsData(branch: String, repo: String, isMain: Bool) -> Data
    func prCommitsData(branch: String, repo: String, base: String, isMain: Bool) -> Data
    func prDiffData(branch: String, repo: String, isMain: Bool) -> Data
}

protocol JiraAPI: PomBaseAPI {
    func jiraBoardsData() -> Data
    func jiraSprintData(board: Int) -> Data
    func jiraIssueData(key: String) -> Data
    func jiraIssuesData(branches: [String]) -> Data
}

protocol SessionAPI: PomBaseAPI {
    func sessionListData() -> Data
    func sessionSwitch(name: String) -> Data
    func sessionDelete(name: String, purge: Bool) -> Data
    func sessionCreate(json: String) -> Data
}

protocol BundleAPI: PomBaseAPI {
    func bundleExport(includeSecrets: Bool, password: String) -> Data
    func bundleRead(dataB64: String, password: String) -> Data
    func bundleApply(dataB64: String, password: String, yaml: String, writeConfig: Bool, createSecrets: Bool) -> Data
    func bundleAdapt(dataB64: String, password: String, yaml: String, createSecrets: Bool) -> Data
}

protocol CoreAPI: PomBaseAPI {
    func logsData() -> Data
    func nmStoreListData() -> Data
    func nmStoreDelete(repo: String, hash: String) -> Data
    func nmStoreReconcile() -> Data
    func nmStoreReclaim() -> Data
    func nmStoreProgress() -> String
    func codeAgentsData() -> Data
    func syncGetData() -> Data
    func syncSet(refreshMain: Bool, intervalSec: Int) -> Data
    func fetchImageData(url: String) -> Data
}

protocol PomAPI: WorkspaceAPI, TerminalAPI, ServiceAPI, ConfigAPI, DBAPI,
    SharedServiceAPI, SecretsAPI, PRAPI, JiraAPI, SessionAPI, BundleAPI, CoreAPI {}

extension PomCore: PomAPI {}
