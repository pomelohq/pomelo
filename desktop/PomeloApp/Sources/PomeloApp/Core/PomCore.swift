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


    func workspacesData(git: Bool) -> Data {
        guard let out = PomWorkspaces(git ? 1 : 0) else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }
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

    func peekAllData(windows: [String], lines: Int) -> Data {
        windows.joined(separator: ",").withCString { c in
            guard let out = PomPeekAll(UnsafeMutablePointer(mutating: c), Int32(lines)) else { return Data() }
            defer { PomFree(out) }
            return Data(String(cString: out).utf8)
        }
    }

    func doctorData() -> Data {
        guard let out = PomDoctor() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func configFilesData() -> Data {
        guard let out = PomConfigFiles() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func logsData() -> Data {
        guard let out = PomLogs() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func configFileGetData(path: String) -> Data { query(domain: "config_get", params: jp(["path": path])) }
    @discardableResult
    func configFileSet(path: String, yaml: String, dry: Bool) -> Data {
        path.withCString { p in yaml.withCString { y in
            cstr(PomConfigFileSet(UnsafeMutablePointer(mutating: p), UnsafeMutablePointer(mutating: y), dry ? 1 : 0))
        }}
    }
    @discardableResult
    func configFileCreate(name: String, yaml: String) -> Data {
        name.withCString { n in yaml.withCString { y in
            cstr(PomConfigFileCreate(UnsafeMutablePointer(mutating: n), UnsafeMutablePointer(mutating: y)))
        }}
    }
    @discardableResult
    func installDeps(branch: String, isMain: Bool) -> Data { command(domain: "deps", action: "install", params: jp(["branch": branch, "is_main": isMain])) }
    @discardableResult
    func configReload() -> Data { command(domain: "config", action: "reload", params: Data("{}".utf8)) }

    func nmStoreListData() -> Data {
        guard let out = PomNMStoreList() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func nmStoreDelete(repo: String, hash: String) -> Data {
        repo.withCString { r in hash.withCString { h in
            guard let out = PomNMStoreDelete(UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: h)) else { return Data() }
            defer { PomFree(out) }
            return Data(String(cString: out).utf8)
        }}
    }

    func nmStoreReconcile() -> Data { command(domain: "nmstore", action: "reconcile", params: Data("{}".utf8)) }

    func nmStoreReclaim() -> Data { command(domain: "nmstore", action: "reclaim", params: Data("{}".utf8)) }

    func nmStoreProgress() -> String {
        guard let out = PomNMStoreProgress() else { return "" }
        defer { PomFree(out) }
        return String(cString: out)
    }

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

    func syncGetData() -> Data {
        guard let out = PomSyncGet() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

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
    func dbExportCSV(branch: String, db: String, sql: String, path: String) -> Data {
        branch.withCString { b in db.withCString { d in sql.withCString { q in path.withCString { p in
            cstr(PomDBExportCSV(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: d),
                                UnsafeMutablePointer(mutating: q), UnsafeMutablePointer(mutating: p)))
        }}}}
    }
    func dbConsolesLoadData() -> Data { query(domain: "db_consoles_load", params: Data("{}".utf8)) }
    @discardableResult
    func dbConsolesSave(json: String) -> Data { json.withCString { cstr(PomDBConsolesSave(UnsafeMutablePointer(mutating: $0))) } }
    func sharedLogsData(name: String, lines: Int) -> Data { query(domain: "shared_logs", params: jp(["name": name, "lines": String(lines)])) }
    @discardableResult
    func sharedAction(name: String, action: String) -> Data { command(domain: "shared", action: action, params: jp(["name": name])) }
    @discardableResult
    func sharedStack(action: String) -> Data { command(domain: "shared_stack", action: action, params: Data("{}".utf8)) }
    @discardableResult
    func openEditor(branch: String, isMain: Bool, repo: String, editor: String, resolveOnly: Bool) -> Data {
        branch.withCString { b in repo.withCString { r in editor.withCString { e in
            cstr(PomOpenEditor(UnsafeMutablePointer(mutating: b), isMain ? 1 : 0, UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: e), resolveOnly ? 1 : 0))
        }}}
    }
    @discardableResult
    func claudeTerminal(branch: String, isMain: Bool) -> Data { query(domain: "claude_terminal", params: jp(["branch": branch, "is_main": isMain])) }
    func bundleExport(includeSecrets: Bool, password: String) -> Data {
        password.withCString { p in cstr(PomBundleExport(includeSecrets ? 1 : 0, UnsafeMutablePointer(mutating: p))) }
    }
    func bundleRead(dataB64: String, password: String) -> Data {
        dataB64.withCString { d in password.withCString { p in
            cstr(PomBundleRead(UnsafeMutablePointer(mutating: d), UnsafeMutablePointer(mutating: p)))
        }}
    }
    @discardableResult
    func bundleApply(dataB64: String, password: String, yaml: String, writeConfig: Bool, createSecrets: Bool) -> Data {
        dataB64.withCString { d in password.withCString { p in yaml.withCString { y in
            cstr(PomBundleApply(UnsafeMutablePointer(mutating: d), UnsafeMutablePointer(mutating: p), UnsafeMutablePointer(mutating: y), writeConfig ? 1 : 0, createSecrets ? 1 : 0))
        }}}
    }
    @discardableResult
    func bundleAdapt(dataB64: String, password: String, yaml: String, createSecrets: Bool) -> Data {
        dataB64.withCString { d in password.withCString { p in yaml.withCString { y in
            cstr(PomBundleAdapt(UnsafeMutablePointer(mutating: d), UnsafeMutablePointer(mutating: p), UnsafeMutablePointer(mutating: y), createSecrets ? 1 : 0))
        }}}
    }
    func configExplainData(repo: String, branch: String, svc: String, env: String) -> Data { query(domain: "config_explain", params: jp(["repo": repo, "branch": branch, "svc": svc, "env": env])) }
    func environmentsData() -> Data { query(domain: "environments", params: Data("{}".utf8)) }
    func suggestName(branch: String, desc: String) -> Data { query(domain: "suggest_name", params: jp(["branch": branch, "desc": desc])) }
    @discardableResult
    func serviceControl(refJSON: String, action: String) -> Data {
        refJSON.withCString { r in action.withCString { a in
            cstr(PomServiceControl(UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: a)))
        }}
    }
    @discardableResult
    func shortcutRun(branch: String, isMain: Bool, repo: String, cmd: String) -> Data {
        branch.withCString { b in repo.withCString { r in cmd.withCString { c in
            cstr(PomShortcutRun(UnsafeMutablePointer(mutating: b), isMain ? 1 : 0, UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: c)))
        }}}
    }
    @discardableResult
    func envSet(branch: String, isMain: Bool, repo: String, svc: String, env: String) -> Data {
        branch.withCString { b in repo.withCString { r in svc.withCString { sv in env.withCString { e in
            cstr(PomEnvSet(UnsafeMutablePointer(mutating: b), isMain ? 1 : 0, UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: sv), UnsafeMutablePointer(mutating: e)))
        }}}}
    }
    @discardableResult
    func serviceMode(repo: String, svc: String, mode: String) -> Data {
        repo.withCString { r in svc.withCString { sv in mode.withCString { m in
            cstr(PomServiceMode(UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: sv), UnsafeMutablePointer(mutating: m)))
        }}}
    }
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
    func devProxyLogData(limit: Int) -> Data { cstr(PomDevProxyLog(Int32(limit))) }
    func paneBusyData(holder: String) -> Data { query(domain: "pane_busy", params: jp(["holder": holder])) }
    func sessionListData() -> Data { query(domain: "session_list", params: Data("{}".utf8)) }
    @discardableResult
    func sessionSwitch(name: String) -> Data { command(domain: "session", action: "switch", params: jp(["name": name])) }
    @discardableResult
    func sessionDelete(name: String, purge: Bool) -> Data { command(domain: "session", action: "delete", params: jp(["name": name, "purge": purge])) }
    @discardableResult
    func sessionCreate(json: String) -> Data { json.withCString { cstr(PomSessionCreate(UnsafeMutablePointer(mutating: $0))) } }
    func integrationsStatusData() -> Data { query(domain: "integrations_status", params: Data("{}".utf8)) }
    func secretNamesData() -> Data { query(domain: "secret_names", params: Data("{}".utf8)) }
    func secretGet(name: String) -> Data {
        name.withCString { n in cstr(PomSecretGet(UnsafeMutablePointer(mutating: n))) }
    }
    @discardableResult
    func secretSet(name: String, value: String) -> Data { command(domain: "secret", action: "set", params: jp(["name": name, "value": value])) }
    @discardableResult
    func jiraSet(site: String, email: String, token: String) -> Data { command(domain: "jira", action: "set", params: jp(["site": site, "email": email, "token": token])) }
    func versionData() -> Data { cstr(PomVersion()) }
    func psData() -> Data { query(domain: "ps", params: Data("{}".utf8)) }
    @discardableResult
    func mainPull(branch: String) -> Data {
        branch.withCString { b in cstr(PomMainPull(UnsafeMutablePointer(mutating: b))) }
    }
    @discardableResult
    func gitPull(branch: String, repo: String, isMain: Bool) -> Data {
        branch.withCString { b in repo.withCString { r in
            cstr(PomGitPull(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: r), isMain ? 1 : 0))
        }}
    }
    func githubTest(token: String) -> Data {
        token.withCString { t in cstr(PomGithubTest(UnsafeMutablePointer(mutating: t))) }
    }
    func prAllData() -> Data { cstr(PomPRAll()) }
    func prWorkspaceData(branch: String, isMain: Bool) -> Data {
        branch.withCString { b in cstr(PomPRWorkspace(UnsafeMutablePointer(mutating: b), isMain ? 1 : 0)) }
    }
    func prDetailData(branch: String, repo: String, isMain: Bool) -> Data {
        branch.withCString { b in repo.withCString { r in
            cstr(PomPRDetail(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: r), isMain ? 1 : 0))
        }}
    }
    func prCommentsData(branch: String, repo: String, isMain: Bool) -> Data {
        branch.withCString { b in repo.withCString { r in
            cstr(PomPRComments(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: r), isMain ? 1 : 0))
        }}
    }
    func prCommitsData(branch: String, repo: String, base: String, isMain: Bool) -> Data {
        branch.withCString { b in repo.withCString { r in base.withCString { ba in
            cstr(PomPRCommits(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: ba), isMain ? 1 : 0))
        }}}
    }
    func prDiffData(branch: String, repo: String, isMain: Bool) -> Data {
        branch.withCString { b in repo.withCString { r in
            cstr(PomPRDiff(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: r), isMain ? 1 : 0))
        }}
    }
    func localChangesData(branch: String, isMain: Bool) -> Data {
        branch.withCString { b in cstr(PomWorkspaceLocalChanges(UnsafeMutablePointer(mutating: b), isMain ? 1 : 0)) }
    }
    func localDiffData(branch: String, repo: String, isMain: Bool) -> Data {
        branch.withCString { b in repo.withCString { r in
            cstr(PomLocalDiff(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: r), isMain ? 1 : 0))
        }}
    }
    func jiraTest(site: String, email: String, token: String) -> Data {
        site.withCString { s in email.withCString { e in token.withCString { t in
            cstr(PomJiraTest(UnsafeMutablePointer(mutating: s), UnsafeMutablePointer(mutating: e), UnsafeMutablePointer(mutating: t)))
        }}}
    }
    func jiraBoardsData() -> Data { query(domain: "jira_boards", params: Data("{}".utf8)) }
    func jiraSprintData(board: Int) -> Data { query(domain: "jira_sprint", params: jp(["board": board])) }
    func jiraIssueData(key: String) -> Data { query(domain: "jira_issue", params: jp(["key": key])) }
    func jiraIssuesData(branches: [String]) -> Data { query(domain: "jira_issues", params: jp(["branches": branches])) }

    private func cstr(_ out: UnsafeMutablePointer<CChar>?) -> Data {
        guard let out else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }
}
