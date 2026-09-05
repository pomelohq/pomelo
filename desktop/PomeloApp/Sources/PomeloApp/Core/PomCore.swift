import Foundation
import CPom

final class PomCore: @unchecked Sendable {
    static let shared = PomCore()

    private(set) var session: String = ""
    private(set) var ready = false
    private(set) var initError: String?

    private init() {}

    static func resolveConfigPath() -> String? {
        if let env = ProcessInfo.processInfo.environment["POM_CONFIG"], !env.isEmpty {
            return env
        }
        let home = FileManager.default.homeDirectoryForCurrentUser
        let last = home.appendingPathComponent(".local/state/pom/last_project")
        if let dir = try? String(contentsOf: last, encoding: .utf8) {
            let trimmed = dir.trimmingCharacters(in: .whitespacesAndNewlines)
            for name in ["pom.yml"] {
                let p = (trimmed as NSString).appendingPathComponent(name)
                if FileManager.default.fileExists(atPath: p) { return p }
            }
        }
        return nil
    }

    func start(configPath: String) {
        let res = configPath.withCString { cstr -> String in
            guard let out = PomInit(UnsafeMutablePointer(mutating: cstr)) else { return "error: nil" }
            defer { PomFree(out) }
            return String(cString: out)
        }
        if res.hasPrefix("ok:") {
            session = String(res.dropFirst(3))
            ready = true
        } else {
            initError = res
        }
    }

    func updateSession(_ s: String) { session = s }


    func workspacesData(git: Bool) -> Data { query(domain: "workspaces", params: jp(["git": git])) }
    func livenessData() -> Data { query(domain: "liveness", params: Data("{}".utf8)) }

    func query(domain: String, params: Data) -> Data {
        let p = String(decoding: params, as: UTF8.self)
        return domain.withCString { d in p.withCString { pc in
            cstr(PomQuery(UnsafeMutablePointer(mutating: d), UnsafeMutablePointer(mutating: pc)))
        }}
    }

    func command(domain: String, action: String, params: Data) -> Data {
        let p = String(decoding: params, as: UTF8.self)
        return domain.withCString { d in action.withCString { a in p.withCString { pc in
            cstr(PomCommand(UnsafeMutablePointer(mutating: d), UnsafeMutablePointer(mutating: a), UnsafeMutablePointer(mutating: pc)))
        }}}
    }

    private func jp(_ d: [String: Any]) -> Data { (try? JSONSerialization.data(withJSONObject: d)) ?? Data("{}".utf8) }

    func fetch(domain: String, params: Data) -> Data {
        let p = String(decoding: params, as: UTF8.self)
        return domain.withCString { d in p.withCString { pc in
            cstr(PomFetch(UnsafeMutablePointer(mutating: d), UnsafeMutablePointer(mutating: pc)))
        }}
    }

    func peekAllData(windows: [String], lines: Int) -> Data { query(domain: "peek_all", params: jp(["windows": windows, "lines": lines])) }

    func doctorData() -> Data { query(domain: "doctor", params: Data("{}".utf8)) }

    func configFilesData() -> Data { query(domain: "config_files", params: Data("{}".utf8)) }

    func logsData() -> Data { query(domain: "logs", params: Data("{}".utf8)) }

    func configFileGetData(path: String) -> Data { query(domain: "config_get", params: jp(["path": path])) }
    @discardableResult
    func configFileSet(path: String, yaml: String, dry: Bool) -> Data { command(domain: "config", action: "file_set", params: jp(["path": path, "yaml": yaml, "dry": dry])) }
    @discardableResult
    func configFileCreate(name: String, yaml: String) -> Data { command(domain: "config", action: "file_create", params: jp(["name": name, "yaml": yaml])) }
    @discardableResult
    func installDeps(branch: String, isMain: Bool) -> Data { command(domain: "deps", action: "install", params: jp(["branch": branch, "is_main": isMain])) }
    @discardableResult
    func configReload() -> Data { command(domain: "config", action: "reload", params: Data("{}".utf8)) }

    func nmStoreListData() -> Data { query(domain: "nmstore_list", params: Data("{}".utf8)) }

    func nmStoreDelete(repo: String, hash: String) -> Data { command(domain: "nmstore", action: "delete", params: jp(["repo": repo, "hash": hash])) }

    func nmStoreReconcile() -> Data { command(domain: "nmstore", action: "reconcile", params: Data("{}".utf8)) }

    func nmStoreReclaim() -> Data { command(domain: "nmstore", action: "reclaim", params: Data("{}".utf8)) }

    func nmStoreProgress() -> String { String(decoding: fetch(domain: "nmstore_progress", params: Data("{}".utf8)), as: UTF8.self) }

    func codeAgentsData() -> Data {
        guard let out = PomCodeAgents() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func claudeUsageData() -> Data {
        guard let out = PomClaudeUsage() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func syncGetData() -> Data { query(domain: "sync_get", params: Data("{}".utf8)) }

    @discardableResult
    func syncSet(refreshMain: Bool, intervalSec: Int) -> Data { command(domain: "sync", action: "set", params: jp(["refresh_main": refreshMain, "interval_sec": intervalSec])) }

    func sharedStatusData() -> Data { query(domain: "shared_status", params: Data("{}".utf8)) }
    func mcpStatusData() -> Data { query(domain: "mcp_status", params: Data("{}".utf8)) }
    func mcpReinstallData() -> Data { command(domain: "mcp", action: "reinstall", params: Data("{}".utf8)) }
    func fetchImageData(url: String) -> Data { query(domain: "fetch_image", params: jp(["url": url])) }
    func sharedStatsData(name: String) -> Data { query(domain: "shared_stats", params: jp(["name": name])) }
    func sharedInspectData(name: String) -> Data { query(domain: "shared_inspect", params: jp(["name": name])) }
    func dbListData(branch: String) -> Data { query(domain: "db_list", params: jp(["branch": branch])) }
    func dbTablesData(branch: String, db: String) -> Data { query(domain: "db_tables", params: jp(["branch": branch, "db": db])) }
    func dbQueryData(branch: String, db: String, sql: String, limit: Int) -> Data { query(domain: "db_query", params: jp(["branch": branch, "db": db, "sql": sql, "limit": limit])) }
    func dbColumnsData(branch: String, db: String) -> Data { query(domain: "db_columns", params: jp(["branch": branch, "db": db])) }
    func dbExportCSV(branch: String, db: String, sql: String, path: String) -> Data { command(domain: "db", action: "export_csv", params: jp(["branch": branch, "db": db, "sql": sql, "path": path])) }
    func dbConsolesLoadData() -> Data { query(domain: "db_consoles_load", params: Data("{}".utf8)) }
    @discardableResult
    func dbConsolesSave(json: String) -> Data { command(domain: "db", action: "consoles_save", params: jp(["data": json])) }
    func sharedLogsData(name: String, lines: Int) -> Data { query(domain: "shared_logs", params: jp(["name": name, "lines": String(lines)])) }
    @discardableResult
    func sharedAction(name: String, action: String) -> Data { command(domain: "shared", action: action, params: jp(["name": name])) }
    @discardableResult
    func sharedStack(action: String) -> Data { command(domain: "shared_stack", action: action, params: Data("{}".utf8)) }
    @discardableResult
    func openEditor(branch: String, isMain: Bool, repo: String, editor: String, resolveOnly: Bool) -> Data { command(domain: "editor", action: "open", params: jp(["branch": branch, "is_main": isMain, "repo": repo, "editor": editor, "resolve_only": resolveOnly])) }
    @discardableResult
    func claudeTerminal(branch: String, isMain: Bool) -> Data { query(domain: "claude_terminal", params: jp(["branch": branch, "is_main": isMain])) }
    func bundleExport(includeSecrets: Bool, password: String) -> Data { command(domain: "bundle", action: "export", params: jp(["include_secrets": includeSecrets, "password": password])) }
    func bundleRead(dataB64: String, password: String) -> Data { command(domain: "bundle", action: "read", params: jp(["data_b64": dataB64, "password": password])) }
    @discardableResult
    func bundleApply(dataB64: String, password: String, yaml: String, writeConfig: Bool, createSecrets: Bool) -> Data { command(domain: "bundle", action: "apply", params: jp(["data_b64": dataB64, "password": password, "yaml": yaml, "write_config": writeConfig, "create_secrets": createSecrets])) }
    @discardableResult
    func bundleAdapt(dataB64: String, password: String, yaml: String, createSecrets: Bool) -> Data { command(domain: "bundle", action: "adapt", params: jp(["data_b64": dataB64, "password": password, "yaml": yaml, "create_secrets": createSecrets])) }
    func configExplainData(repo: String, branch: String, svc: String, env: String) -> Data { query(domain: "config_explain", params: jp(["repo": repo, "branch": branch, "svc": svc, "env": env])) }
    func environmentsData() -> Data { query(domain: "environments", params: Data("{}".utf8)) }
    func suggestName(branch: String, desc: String) -> Data { query(domain: "suggest_name", params: jp(["branch": branch, "desc": desc])) }
    @discardableResult
    func serviceControl(refJSON: String, action: String) -> Data { command(domain: "service", action: action, params: Data(refJSON.utf8)) }
    @discardableResult
    func shortcutRun(branch: String, isMain: Bool, repo: String, cmd: String) -> Data { command(domain: "shortcut", action: "run", params: jp(["branch": branch, "is_main": isMain, "repo": repo, "cmd": cmd])) }
    @discardableResult
    func envSet(branch: String, isMain: Bool, repo: String, svc: String, env: String) -> Data { command(domain: "service_env", action: "set", params: jp(["branch": branch, "is_main": isMain, "repo": repo, "svc": svc, "env": env])) }
    @discardableResult
    func serviceMode(repo: String, svc: String, mode: String) -> Data { command(domain: "service_mode", action: "set", params: jp(["repo": repo, "svc": svc, "mode": mode])) }
    func serviceURL(branch: String, repo: String, svc: String) -> Data { query(domain: "service_url", params: jp(["branch": branch, "repo": repo, "svc": svc])) }
    @discardableResult
    func ptyReap() -> Data { command(domain: "pty", action: "reap", params: Data("{}".utf8)) }
    @discardableResult
    func paneKill(paneID: String) -> Data { command(domain: "pane", action: "kill", params: jp(["pane_id": paneID])) }
    @discardableResult
    func workspaceRename(branch: String, isMain: Bool, displayName: String) -> Data { command(domain: "workspace", action: "rename", params: jp(["branch": branch, "is_main": isMain, "display_name": displayName])) }
    func editorsData() -> Data { query(domain: "editors", params: Data("{}".utf8)) }
    func networkData() -> Data { query(domain: "network", params: Data("{}".utf8)) }
    @discardableResult
    func networkSetPorts(proxyPort: Int, webhookPort: Int) -> Data { command(domain: "network", action: "set_ports", params: jp(["proxy_port": proxyPort, "webhook_port": webhookPort])) }
    @discardableResult
    func networkStart() -> Data { command(domain: "network", action: "start", params: Data("{}".utf8)) }
    func remoteInfoData() -> Data { query(domain: "remote_info", params: Data("{}".utf8)) }
    func remoteSet(enabled: Bool) -> Data { command(domain: "remote", action: "set", params: jp(["enabled": enabled])) }
    func devProxyLogData(limit: Int) -> Data { query(domain: "devproxy_log", params: jp(["limit": limit])) }
    func paneBusyData(holder: String) -> Data { query(domain: "pane_busy", params: jp(["holder": holder])) }
    func sessionListData() -> Data { query(domain: "session_list", params: Data("{}".utf8)) }
    @discardableResult
    func sessionSwitch(name: String) -> Data { command(domain: "session", action: "switch", params: jp(["name": name])) }
    @discardableResult
    func sessionDelete(name: String, purge: Bool) -> Data { command(domain: "session", action: "delete", params: jp(["name": name, "purge": purge])) }
    @discardableResult
    func sessionCreate(json: String) -> Data { command(domain: "session", action: "create", params: Data(json.utf8)) }
    func integrationsStatusData() -> Data { query(domain: "integrations_status", params: Data("{}".utf8)) }
    func secretNamesData() -> Data { query(domain: "secret_names", params: Data("{}".utf8)) }
    func secretGet(name: String) -> Data { query(domain: "secret_get", params: jp(["name": name])) }
    @discardableResult
    func secretSet(name: String, value: String) -> Data { command(domain: "secret", action: "set", params: jp(["name": name, "value": value])) }
    @discardableResult
    func jiraSet(site: String, email: String, token: String) -> Data { command(domain: "jira", action: "set", params: jp(["site": site, "email": email, "token": token])) }
    func versionData() -> Data { query(domain: "version", params: Data("{}".utf8)) }
    func psData() -> Data { query(domain: "ps", params: Data("{}".utf8)) }
    @discardableResult
    func mainPull(branch: String) -> Data { command(domain: "git", action: "main_pull", params: jp(["branch": branch])) }
    @discardableResult
    func gitPull(branch: String, repo: String, isMain: Bool) -> Data { command(domain: "git", action: "pull", params: jp(["branch": branch, "repo": repo, "is_main": isMain])) }
    func githubTest(token: String) -> Data { command(domain: "github", action: "test", params: jp(["token": token])) }
    func gitStatusData(branch: String, isMain: Bool) -> Data { query(domain: "git_status", params: jp(["branch": branch, "is_main": isMain])) }
    @discardableResult
    func gitStage(branch: String, repo: String, isMain: Bool, paths: [String]) -> Data { command(domain: "git", action: "stage", params: jp(["branch": branch, "repo": repo, "is_main": isMain, "paths": paths])) }
    @discardableResult
    func gitUnstage(branch: String, repo: String, isMain: Bool, paths: [String]) -> Data { command(domain: "git", action: "unstage", params: jp(["branch": branch, "repo": repo, "is_main": isMain, "paths": paths])) }
    @discardableResult
    func gitDiscard(branch: String, repo: String, isMain: Bool, paths: [String]) -> Data { command(domain: "git", action: "discard", params: jp(["branch": branch, "repo": repo, "is_main": isMain, "paths": paths])) }
    @discardableResult
    func gitCommit(branch: String, repo: String, isMain: Bool, message: String) -> Data { command(domain: "git", action: "commit", params: jp(["branch": branch, "repo": repo, "is_main": isMain, "message": message])) }
    @discardableResult
    func gitPush(branch: String, repo: String, isMain: Bool) -> Data { command(domain: "git", action: "push", params: jp(["branch": branch, "repo": repo, "is_main": isMain])) }
    func prAllData() -> Data { fetch(domain: "pr_all", params: Data("{}".utf8)) }
    func prWorkspaceData(branch: String, isMain: Bool) -> Data { fetch(domain: "pr_workspace", params: jp(["branch": branch, "is_main": isMain])) }
    func prDetailData(branch: String, repo: String, isMain: Bool) -> Data { fetch(domain: "pr_detail", params: jp(["branch": branch, "repo": repo, "is_main": isMain])) }
    func prCommentsData(branch: String, repo: String, isMain: Bool) -> Data { fetch(domain: "pr_comments", params: jp(["branch": branch, "repo": repo, "is_main": isMain])) }
    func prTimelineData(branch: String, repo: String, isMain: Bool) -> Data { fetch(domain: "pr_timeline", params: jp(["branch": branch, "repo": repo, "is_main": isMain])) }
    func prRefresh() -> Data { command(domain: "pr", action: "refresh", params: Data("{}".utf8)) }
    func prCommitsData(branch: String, repo: String, base: String, isMain: Bool) -> Data { query(domain: "pr_commits", params: jp(["branch": branch, "repo": repo, "base": base, "is_main": isMain])) }
    func prDiffData(branch: String, repo: String, isMain: Bool) -> Data { fetch(domain: "pr_diff", params: jp(["branch": branch, "repo": repo, "is_main": isMain])) }
    func reviewGetData(branch: String, isMain: Bool) -> Data { fetch(domain: "review_get", params: jp(["branch": branch, "is_main": isMain])) }
    func filePeekData(branch: String, repo: String, path: String, isMain: Bool) -> Data { fetch(domain: "file_peek", params: jp(["branch": branch, "repo": repo, "path": path, "is_main": isMain])) }
    func workspaceFilesData(branch: String, isMain: Bool) -> Data { fetch(domain: "workspace_files", params: jp(["branch": branch, "is_main": isMain])) }
    func fileReadData(branch: String, repo: String, path: String, isMain: Bool) -> Data { fetch(domain: "file_read", params: jp(["branch": branch, "repo": repo, "path": path, "is_main": isMain])) }
    func reviewThreadsData(branch: String, isMain: Bool) -> Data { fetch(domain: "review_threads", params: jp(["branch": branch, "is_main": isMain])) }
    func reviewThreadAdd(branch: String, isMain: Bool, repo: String, path: String, start: Int, end: Int, side: String, body: String) -> Data {
        command(domain: "review", action: "thread_add", params: jp(["branch": branch, "is_main": isMain, "repo": repo, "path": path, "start": start, "end": end, "side": side, "body": body]))
    }
    func reviewThreadReply(branch: String, isMain: Bool, id: String, body: String) -> Data { command(domain: "review", action: "thread_reply", params: jp(["branch": branch, "is_main": isMain, "id": id, "body": body])) }
    func reviewThreadResolve(branch: String, isMain: Bool, id: String, resolved: Bool) -> Data { command(domain: "review", action: "thread_resolve", params: jp(["branch": branch, "is_main": isMain, "id": id, "resolved": resolved])) }
    func localChangesData(branch: String, isMain: Bool) -> Data { fetch(domain: "local_changes", params: jp(["branch": branch, "is_main": isMain])) }
    func prCommitDiffData(branch: String, repo: String, sha: String, isMain: Bool) -> Data { fetch(domain: "pr_commit_diff", params: jp(["branch": branch, "repo": repo, "sha": sha, "is_main": isMain])) }
    func localDiffData(branch: String, repo: String, isMain: Bool) -> Data { fetch(domain: "local_diff", params: jp(["branch": branch, "repo": repo, "is_main": isMain]))
    }
    func jiraTest(site: String, email: String, token: String) -> Data { command(domain: "jira", action: "test", params: jp(["site": site, "email": email, "token": token])) }
    func jiraBoardsData() -> Data { query(domain: "jira_boards", params: Data("{}".utf8)) }
    func jiraSprintData(board: Int) -> Data { query(domain: "jira_sprint", params: jp(["board": board])) }
    func jiraIssueData(key: String, force: Bool = false) -> Data { query(domain: "jira_issue", params: jp(["key": key, "force": force])) }
    func jiraIssuesData(branches: [String]) -> Data { query(domain: "jira_issues", params: jp(["branches": branches])) }

    private func cstr(_ out: UnsafeMutablePointer<CChar>?) -> Data {
        guard let out else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }
}
