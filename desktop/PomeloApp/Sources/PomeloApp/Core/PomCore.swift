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
    func livenessData() -> Data { cstr(PomLiveness()) }

    func agentStatesData() -> Data {
        guard let out = PomAgentStates() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

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

    func configFileGetData(path: String) -> Data { path.withCString { cstr(PomConfigFileGet(UnsafeMutablePointer(mutating: $0))) } }
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
    func installDeps(branch: String, isMain: Bool) -> Data {
        branch.withCString { b in cstr(PomInstallDeps(UnsafeMutablePointer(mutating: b), isMain ? 1 : 0)) }
    }
    @discardableResult
    func configReload() -> Data { cstr(PomConfigReload()) }

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

    func nmStoreReconcile() -> Data {
        guard let out = PomNMStoreReconcile() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func nmStoreReclaim() -> Data {
        guard let out = PomNMStoreReclaim() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func nmStoreProgress() -> String {
        guard let out = PomNMStoreProgress() else { return "" }
        defer { PomFree(out) }
        return String(cString: out)
    }

    func syncGetData() -> Data {
        guard let out = PomSyncGet() else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    @discardableResult
    func syncSet(refreshMain: Bool, intervalSec: Int) -> Data {
        guard let out = PomSyncSet(refreshMain ? 1 : 0, Int32(intervalSec)) else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }

    func sharedStatusData() -> Data { cstr(PomSharedStatus()) }
    func mcpStatusData() -> Data { cstr(PomMCPStatus()) }
    func mcpReinstallData() -> Data { cstr(PomMCPReinstall()) }
    func fetchImageData(url: String) -> Data { url.withCString { cstr(PomFetchImage(UnsafeMutablePointer(mutating: $0))) } }
    func sharedStatsData(name: String) -> Data { name.withCString { cstr(PomSharedStats(UnsafeMutablePointer(mutating: $0))) } }
    func sharedInspectData(name: String) -> Data { name.withCString { cstr(PomSharedInspect(UnsafeMutablePointer(mutating: $0))) } }
    func dbListData(branch: String) -> Data { branch.withCString { cstr(PomDBList(UnsafeMutablePointer(mutating: $0))) } }
    func dbTablesData(branch: String, db: String) -> Data {
        branch.withCString { b in db.withCString { d in
            cstr(PomDBTables(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: d)))
        }}
    }
    func dbQueryData(branch: String, db: String, sql: String, limit: Int) -> Data {
        branch.withCString { b in db.withCString { d in sql.withCString { q in
            cstr(PomDBQuery(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: d), UnsafeMutablePointer(mutating: q), Int32(limit)))
        }}}
    }
    func dbColumnsData(branch: String, db: String) -> Data {
        branch.withCString { b in db.withCString { d in
            cstr(PomDBColumns(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: d)))
        }}
    }
    func dbExportCSV(branch: String, db: String, sql: String, path: String) -> Data {
        branch.withCString { b in db.withCString { d in sql.withCString { q in path.withCString { p in
            cstr(PomDBExportCSV(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: d),
                                UnsafeMutablePointer(mutating: q), UnsafeMutablePointer(mutating: p)))
        }}}}
    }
    func dbConsolesLoadData() -> Data { cstr(PomDBConsolesLoad()) }
    @discardableResult
    func dbConsolesSave(json: String) -> Data { json.withCString { cstr(PomDBConsolesSave(UnsafeMutablePointer(mutating: $0))) } }
    func sharedLogsData(name: String, lines: Int) -> Data {
        name.withCString { n in String(lines).withCString { l in
            cstr(PomSharedLogs(UnsafeMutablePointer(mutating: n), UnsafeMutablePointer(mutating: l)))
        }}
    }
    @discardableResult
    func sharedAction(name: String, action: String) -> Data {
        name.withCString { n in action.withCString { a in
            cstr(PomSharedAction(UnsafeMutablePointer(mutating: n), UnsafeMutablePointer(mutating: a)))
        }}
    }
    @discardableResult
    func sharedStack(action: String) -> Data { action.withCString { cstr(PomSharedStack(UnsafeMutablePointer(mutating: $0))) } }
    @discardableResult
    func openEditor(branch: String, isMain: Bool, repo: String, editor: String, resolveOnly: Bool) -> Data {
        branch.withCString { b in repo.withCString { r in editor.withCString { e in
            cstr(PomOpenEditor(UnsafeMutablePointer(mutating: b), isMain ? 1 : 0, UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: e), resolveOnly ? 1 : 0))
        }}}
    }
    @discardableResult
    func claudeTerminal(branch: String, isMain: Bool) -> Data {
        branch.withCString { cstr(PomClaudeTerminal(UnsafeMutablePointer(mutating: $0), isMain ? 1 : 0)) }
    }
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
    func configExplainData(repo: String, branch: String, svc: String, env: String) -> Data {
        repo.withCString { r in branch.withCString { b in svc.withCString { sv in env.withCString { e in
            cstr(PomConfigExplain(UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: sv), UnsafeMutablePointer(mutating: e)))
        }}}}
    }
    func environmentsData() -> Data { cstr(PomEnvironments()) }
    func suggestName(branch: String, desc: String) -> Data {
        branch.withCString { b in desc.withCString { d in
            cstr(PomSuggestName(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: d)))
        }}
    }
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
    func serviceURL(branch: String, repo: String, svc: String) -> Data {
        branch.withCString { b in repo.withCString { r in svc.withCString { sv in
            cstr(PomServiceURL(UnsafeMutablePointer(mutating: b), UnsafeMutablePointer(mutating: r), UnsafeMutablePointer(mutating: sv)))
        }}}
    }
    @discardableResult
    func ptyReap() -> Data { cstr(PomPtyReap()) }
    @discardableResult
    func paneKill(paneID: String) -> Data { paneID.withCString { cstr(PomPaneKill(UnsafeMutablePointer(mutating: $0))) } }
    @discardableResult
    func workspaceRename(branch: String, isMain: Bool, displayName: String) -> Data {
        branch.withCString { b in displayName.withCString { d in
            cstr(PomWorkspaceRename(UnsafeMutablePointer(mutating: b), isMain ? 1 : 0, UnsafeMutablePointer(mutating: d)))
        }}
    }
    func editorsData() -> Data { cstr(PomEditors()) }
    func networkData() -> Data { cstr(PomNetwork()) }
    @discardableResult
    func networkSetPorts(proxyPort: Int, webhookPort: Int) -> Data { cstr(PomNetworkSetPorts(Int32(proxyPort), Int32(webhookPort))) }
    func devProxyLogData(limit: Int) -> Data { cstr(PomDevProxyLog(Int32(limit))) }
    func paneBusyData(holder: String) -> Data { holder.withCString { cstr(PomPaneBusy(UnsafeMutablePointer(mutating: $0))) } }
    func sessionListData() -> Data { cstr(PomSessionList()) }
    @discardableResult
    func sessionSwitch(name: String) -> Data { name.withCString { cstr(PomSessionSwitch(UnsafeMutablePointer(mutating: $0))) } }
    @discardableResult
    func sessionDelete(name: String, purge: Bool) -> Data { name.withCString { cstr(PomSessionDelete(UnsafeMutablePointer(mutating: $0), purge ? 1 : 0)) } }
    @discardableResult
    func sessionCreate(json: String) -> Data { json.withCString { cstr(PomSessionCreate(UnsafeMutablePointer(mutating: $0))) } }
    func integrationsStatusData() -> Data { cstr(PomIntegrationsStatus()) }
    func secretNamesData() -> Data { cstr(PomSecretNames()) }
    func secretGet(name: String) -> Data {
        name.withCString { n in cstr(PomSecretGet(UnsafeMutablePointer(mutating: n))) }
    }
    @discardableResult
    func secretSet(name: String, value: String) -> Data {
        name.withCString { n in value.withCString { v in
            cstr(PomSecretSet(UnsafeMutablePointer(mutating: n), UnsafeMutablePointer(mutating: v)))
        }}
    }
    @discardableResult
    func jiraSet(site: String, email: String, token: String) -> Data {
        site.withCString { s in email.withCString { e in token.withCString { t in
            cstr(PomJiraSet(UnsafeMutablePointer(mutating: s), UnsafeMutablePointer(mutating: e), UnsafeMutablePointer(mutating: t)))
        }}}
    }
    func versionData() -> Data { cstr(PomVersion()) }
    func psData() -> Data { cstr(PomPs()) }
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
    func jiraTest(site: String, email: String, token: String) -> Data {
        site.withCString { s in email.withCString { e in token.withCString { t in
            cstr(PomJiraTest(UnsafeMutablePointer(mutating: s), UnsafeMutablePointer(mutating: e), UnsafeMutablePointer(mutating: t)))
        }}}
    }
    func jiraBoardsData() -> Data { cstr(PomJiraBoards()) }
    func jiraSprintData(board: Int) -> Data { cstr(PomJiraSprint(Int32(board))) }
    func jiraIssueData(key: String) -> Data {
        key.withCString { k in cstr(PomJiraIssue(UnsafeMutablePointer(mutating: k))) }
    }
    func jiraIssuesData(branches: [String]) -> Data {
        branches.joined(separator: ",").withCString { b in
            cstr(PomJiraIssues(UnsafeMutablePointer(mutating: b)))
        }
    }

    private func cstr(_ out: UnsafeMutablePointer<CChar>?) -> Data {
        guard let out else { return Data() }
        defer { PomFree(out) }
        return Data(String(cString: out).utf8)
    }
}
