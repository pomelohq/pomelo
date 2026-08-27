import Foundation
import SwiftUI
import AppKit
import Combine

@MainActor
final class AppState: ObservableObject {
    private var lastShift: TimeInterval = 0
    private var keysInstalled = false
    weak var uiStore: UIStore?
    weak var themeManager: ThemeManager?

    func installGlobalKeys() {
        guard !keysInstalled else { return }
        keysInstalled = true
        NSEvent.addLocalMonitorForEvents(matching: .flagsChanged) { [weak self] e in
            guard let self, e.keyCode == 56 || e.keyCode == 60 else { return e }
            if e.modifierFlags.contains(.shift) {
                let editing = NSApp.keyWindow?.firstResponder is NSTextView
                if !editing, e.timestamp - self.lastShift < 0.35 {
                    self.showPalette = true; self.lastShift = 0
                } else {
                    self.lastShift = e.timestamp
                }
            }
            return e
        }
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, e.modifierFlags.contains(.command),
                  !e.modifierFlags.contains(.option), !e.modifierFlags.contains(.control) else { return e }
            if (e.charactersIgnoringModifiers ?? "").lowercased() == "q" && !e.modifierFlags.contains(.shift) {
                NSApp.terminate(nil); return nil
            }
            if NSApp.keyWindow?.firstResponder is NSTextView { return e }
            if (e.charactersIgnoringModifiers ?? "") == "," { self.showSettings = true; return nil }  // ⌘, — works without a selection
            let shift = e.modifierFlags.contains(.shift)
            let ch = (e.charactersIgnoringModifiers ?? "").lowercased()
            if shift && (ch == "0" || ch == ")") { self.openActivity(scope: nil); return nil }
            if shift && ch == "s" { self.showShared = true; return nil }
            if shift && ch == "p" { self.showSessionPanel = true; return nil }   // ⌘⇧P — Project (config editor + ENV)
            if shift && ch == "t" { self.themeManager?.cycle(); return nil }      // ⌘⇧T — cycle theme
            if ch == "n" { if shift { self.showCreateSession = true } else { self.openCreateWorkspace = true }; return nil }
            guard let ws = self.selectedWorkspace, let ui = self.uiStore else { return e }
            let ps = ui.state(for: ws.id)
            switch e.charactersIgnoringModifiers ?? "" {
            case "t": ps.newTerminal(); return nil   // ⌘T — new terminal in this workspace
            case "1": ps.pane = .services; return nil
            case "2": if !ws.isMain { ps.pane = .prs }; return nil
            case "3": if !ws.isMain { ps.pane = .jira }; return nil
            case "4": ps.pane = .database; return nil   // Database — valid on main too

            case "0": self.openActivity(scope: ws.id); return nil
            case "i": if !ws.isMain { ps.pane = .claude }; return nil
            case "e": self.openEditor(ws); return nil
            case "j": withAnimation(.easeInOut(duration: 0.16)) { ps.toggleDrawer() }; return nil
            case "b": self.toggleSidebar(); return nil
            case "w": ps.closeSelected(); return nil
            default: return e
            }
        }
    }
    let wsvm = WorkspacesViewModel()
    let prsvm = PRsViewModel()
    let jiravm = JiraViewModel()
    let agentsvm = AgentsViewModel()
    let sessionsvm = SessionsViewModel()
    private var bag = Set<AnyCancellable>()
    init() {
        for vm in [wsvm.objectWillChange, prsvm.objectWillChange, jiravm.objectWillChange, agentsvm.objectWillChange, sessionsvm.objectWillChange] {
            vm.sink { [weak self] _ in self?.objectWillChange.send() }.store(in: &bag)
        }
    }
    var workspaces: [Workspace] { get { wsvm.workspaces } set { wsvm.workspaces = newValue } }
    var selection: String? { get { wsvm.selection } set { wsvm.selection = newValue } }
    @Published var bootError: String?
    @Published var configError: String?

    func refreshConfigHealth() async {
        struct F: Decodable { var id = ""; var severity = ""; var title = ""; var detail = "" }
        struct R: Decodable { var findings: [F] = [] }
        let d = await Task.detached { PomCore.shared.doctorData() }.value
        let findings = (PomJSON.decode(R.self, from: d))?.findings ?? []
        if let f = findings.first(where: { $0.id == "config.validate" && $0.severity == "error" }) {
            configError = f.detail.isEmpty ? f.title : f.detail
        } else {
            configError = nil
        }
    }
    @Published var needsProject = false
    @Published var loading = false
    @Published var sidebarCollapsed = false
    @Published var railCollapsed = false

    private var sidebarToggleAt = Date.distantPast
    func toggleSidebar() {
        let now = Date()
        guard now.timeIntervalSince(sidebarToggleAt) > 0.2 else { return }
        sidebarToggleAt = now
        withAnimation(.easeInOut(duration: 0.16)) { sidebarCollapsed.toggle() }
    }
    var sessions: [SessionItem] { get { sessionsvm.sessions } set { sessionsvm.sessions = newValue } }
    @Published var creating = false
    @Published var showPalette = false
    @Published var appActive = true
    @Published var showActivity = false
    @Published var showPipeline = false
    @Published var updateMainWs: Workspace?
    @Published var fullscreen = false

    @Published var nmBusy = false
    @Published var nmPhase = ""
    @Published var nmSummary: String?

    func nmOptimize(reclaim: Bool) {
        guard !nmBusy else { return }
        nmBusy = true; nmSummary = nil
        nmPhase = reclaim ? "Deduping…" : "Optimizing…"
        let poll = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 250_000_000)
                let p = await Task.detached { PomCore.shared.nmStoreProgress() }.value
                if let self, self.nmBusy, !p.isEmpty { self.nmPhase = p }
            }
        }
        Task { [weak self] in
            let data = await Task.detached { reclaim ? PomCore.shared.nmStoreReclaim() : PomCore.shared.nmStoreReconcile() }.value
            poll.cancel()
            guard let self else { return }
            self.nmSummary = AppState.nmSummaryText(reclaim: reclaim, data: data)
            self.nmPhase = ""; self.nmBusy = false
        }
    }

    private static func nmSummaryText(reclaim: Bool, data: Data) -> String {
        let obj = ((try? JSONSerialization.jsonObject(with: data)) as? [String: Any]) ?? [:]
        func size(_ v: Any?) -> String {
            let n = (v as? NSNumber)?.doubleValue ?? 0
            return n >= 1_073_741_824 ? String(format: "%.2f GB", n / 1_073_741_824) : String(format: "%.0f MB", n / 1_048_576)
        }
        if reclaim {
            let r = (obj["relinked"] as? NSNumber)?.intValue ?? 0
            let rec = (obj["reclaimed"] as? NSNumber)?.int64Value ?? 0
            return rec > 0 ? "Reclaimed \(size(obj["reclaimed"])) (\(r) relinked)" : "Nothing to reclaim"
        }
        let a = (obj["added"] as? NSNumber)?.intValue ?? 0
        return a > 0 ? "Cached \(a) new (\(size(obj["bytes"])))" : "Already optimized"
    }
    var activityScope: String? = nil

    func openActivity(scope: String?) { activityScope = scope; showActivity = true }

    func openEditor(_ ws: Workspace) {
        let b = ws.branch, m = ws.isMain, ed = editorPref
        Task.detached(priority: .userInitiated) {
            PomCore.shared.openEditor(branch: b, isMain: m, repo: "", editor: ed, resolveOnly: false)
        }
    }

    @Published var showSettings = false
    @Published var showShared = false
    @Published var showDependencies = false
    @Published var showSessionPanel = false
    @Published var showCreateSession = false
    @Published var showSessions = false
    @Published var switchError: String?
    @Published var openError: String?
    @Published var openCreateWorkspace = false
    @Published var onboardBranch: String?

    struct RowMenu: Identifiable { let id = UUID(); let ws: Workspace; let at: CGPoint }
    @Published var rowMenu: RowMenu?
    @Published var renamingWs: Workspace?
    @Published var confirmDeleteWs: Workspace?
    @Published var addRepoWs: Workspace?

    @Published var agentModel: AgentStreamModel?
    @Published var showAgentSheet = false
    @Published var showSetup = false
    func maybeShowSetupOnFirstRun() {
        if !UserDefaults.standard.bool(forKey: "didSetupWizard") {
            UserDefaults.standard.set(true, forKey: "didSetupWizard")
            showSetup = true
        }
    }
    func openSetup() { bringMainWindowToFront(); showSetup = true }
    private(set) var agentTitle = "", agentSubtitle = "", agentRunningLabel = ""
    private var agentBag = Set<AnyCancellable>()
    var agentRunning: Bool { agentModel?.running ?? false }

    @Published var agentTarget: String?

    func launchAgent(title: String, subtitle: String, runningLabel: String, branch: String, isMain: Bool = true, role: String, firstTurn: String, target: String? = nil) {
        agentModel?.stop()
        let m = AgentStreamModel()
        agentBag.removeAll()
        m.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }.store(in: &agentBag)
        agentModel = m
        agentTarget = target
        agentTitle = title; agentSubtitle = subtitle; agentRunningLabel = runningLabel
        showAgentSheet = true
        m.start(branch: branch, isMain: isMain, role: role, firstTurn: firstTurn)
        bringMainWindowToFront()
    }

    private func bringMainWindowToFront() {
        NSApp.activate(ignoringOtherApps: true)
        let secondary = ["settings", "create-workspace"]
        let main = NSApp.windows.first { w in
            guard w.isVisible, let id = w.identifier?.rawValue else { return false }
            return !secondary.contains { id.contains($0) }
        } ?? NSApp.windows.first { $0.isVisible }
        main?.makeKeyAndOrderFront(nil)
    }
    func reopenAgent() { if agentModel != nil { showAgentSheet = true } }
    func backgroundAgent() { showAgentSheet = false }

    // Onboarding lives here (not in the sheet) so "Run in background" keeps it
    // alive and a TopBar chip can reopen it.
    @Published var onboardModel: AgentStreamModel?
    @Published var onboardStartAt: Date?
    private(set) var onboardBranchName = "main"
    private var onboardBag = Set<AnyCancellable>()
    var onboardRunning: Bool { onboardModel?.running ?? false }

    func startOnboard(branch: String) {
        onboardBranchName = branch
        if onboardModel == nil {
            let m = AgentStreamModel()
            onboardBag.removeAll()
            m.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }.store(in: &onboardBag)
            onboardModel = m
            onboardStartAt = Date()
            m.start(branch: branch, isMain: true, role: "onboarder", firstTurn: OnboardPrompts.firstTurn)
        }
        onboardBranch = branch
        bringMainWindowToFront()
    }
    func reopenOnboard() { if onboardModel != nil { onboardBranch = onboardBranchName } }
    func endOnboard() {
        onboardModel?.stop(); onboardModel = nil; onboardStartAt = nil; onboardBag.removeAll()
        onboardBranch = nil; Task { await refreshConfigHealth() }
    }
    func endAgent() { agentModel?.stop(); agentModel = nil; agentTarget = nil; agentBag.removeAll(); showAgentSheet = false; Task { await refreshConfigHealth() } }
    @Published var jiraOnlyMine = UserDefaults.standard.bool(forKey: "jiraOnlyMine") {
        didSet { UserDefaults.standard.set(jiraOnlyMine, forKey: "jiraOnlyMine") }
    }
    @Published var editorPref = UserDefaults.standard.string(forKey: "editorPref") ?? "" {
        didSet { UserDefaults.standard.set(editorPref, forKey: "editorPref") }
    }
    @Published var autoPickPort = UserDefaults.standard.object(forKey: "autoPickPort") as? Bool ?? true {
        didSet { UserDefaults.standard.set(autoPickPort, forKey: "autoPickPort") }
    }
    @Published var notifyClaude = UserDefaults.standard.object(forKey: "notifyClaude") as? Bool ?? true {
        didSet { UserDefaults.standard.set(notifyClaude, forKey: "notifyClaude") }
    }
    var agentStates: [String: String] { get { agentsvm.states } set { agentsvm.states = newValue } }
    @Published var ops: [WsOp] = []
    @Published var pendingSvc: [String: String] = [:]
    func svcKey(branch: String, repo: String, svc: String) -> String { "\(branch)|\(repo)|\(svc)" }
    var wsPRs: [String: [WorkspacePR]] { get { prsvm.wsPRs } set { prsvm.wsPRs = newValue } }
    var prsLoading: Bool { get { prsvm.loading } set { prsvm.loading = newValue } }
    var jiraIssues: [String: JiraIssue] { get { jiravm.issues } set { jiravm.issues = newValue } }
    var jiraConfigured: Bool { get { jiravm.configured } set { jiravm.configured = newValue } }

    func prsFor(_ id: String) -> [WorkspacePR] { prsvm.prsFor(id) }
    func prSeverityFor(_ id: String) -> String { prsvm.severityFor(id) }
    func jiraFor(_ branch: String) -> JiraIssue? { jiravm.issueFor(branch) }
    var wsOrder: [String] { get { wsvm.wsOrder } set { wsvm.wsOrder = newValue } }
    @Published var repoOrder: [String] = (UserDefaults.standard.array(forKey: "repoOrder") as? [String]) ?? [] {
        didSet { UserDefaults.standard.set(repoOrder, forKey: "repoOrder") }
    }
    func orderedRepos(_ repos: [Repo]) -> [Repo] {
        let rank = Dictionary(repoOrder.enumerated().map { ($1, $0) }, uniquingKeysWith: { a, _ in a })
        return repos.enumerated().sorted {
            (rank[$0.element.name] ?? 10_000 + $0.offset) < (rank[$1.element.name] ?? 10_000 + $1.offset)
        }.map(\.element)
    }
    func moveRepo(_ dragged: String, before target: String, in names: [String]) {
        guard dragged != target, var arr = Optional(names), let from = arr.firstIndex(of: dragged) else { return }
        arr.remove(at: from)
        let to = arr.firstIndex(of: target) ?? arr.count
        arr.insert(dragged, at: to)
        repoOrder = arr
    }
    func moveRepo(_ dragged: String, toIndex t: Int, in names: [String]) {
        var arr = names
        guard let from = arr.firstIndex(of: dragged) else { return }
        arr.remove(at: from)
        arr.insert(dragged, at: min(max(t, 0), arr.count))
        repoOrder = arr
    }

    var mainWorkspaces: [Workspace] { wsvm.mainWorkspaces }

    var orderedNonMain: [Workspace] { wsvm.orderedNonMain }
    func moveWorkspace(from: IndexSet, to: Int) { wsvm.moveWorkspace(from: from, to: to) }
    func moveWorkspace(_ dragged: String, before target: String) { wsvm.moveWorkspace(dragged, before: target) }
    func moveWorkspace(_ dragged: String, toIndex t: Int) { wsvm.moveWorkspace(dragged, toIndex: t) }
    var allRepoNames: [String] { wsvm.allRepoNames }

    func suggestNameSlug(seed: String, desc: String = "") async -> (name: String, slug: String) {
        struct R: Decodable { var name = ""; var slug = "" }
        let r = await Task.detached(priority: .userInitiated) { () -> R? in
            let d = PomCore.shared.suggestName(branch: seed, desc: desc)
            return PomJSON.decode(R.self, from: d)
        }.value
        let name = (r?.name.trimmingCharacters(in: .whitespaces)).flatMap { $0.isEmpty ? nil : $0 } ?? humanizeBranch(seed)
        let slug = (r?.slug.trimmingCharacters(in: .whitespaces)).flatMap { $0.isEmpty ? nil : $0 } ?? seed
        return (name, slug)
    }

    func suggestName(branch: String, desc: String = "") async -> String {
        await suggestNameSlug(seed: branch, desc: desc).name
    }

    func jiraBoards() async -> [JiraBoard] {
        await Task.detached(priority: .userInitiated) { () -> [JiraBoard] in
            struct R: Decodable { var boards: [JiraBoard]? }
            let d = PomCore.shared.jiraBoardsData()
            return (PomJSON.decode(R.self, from: d)?.boards) ?? []
        }.value
    }

    func jiraIssue(_ key: String) async -> JiraDetail? {
        await Task.detached(priority: .userInitiated) { () -> JiraDetail? in
            let d = PomCore.shared.jiraIssueData(key: key)
            guard let det = PomJSON.decode(JiraDetail.self, from: d), !det.key.isEmpty else { return nil }
            return det
        }.value
    }

    func jiraSprint(board: Int) async -> [SprintIssue] {
        await Task.detached(priority: .userInitiated) { () -> [SprintIssue] in
            struct R: Decodable { var issues: [SprintIssue]? }
            let d = PomCore.shared.jiraSprintData(board: board)
            return (PomJSON.decode(R.self, from: d)?.issues) ?? []
        }.value
    }

    func renameWorkspace(_ ws: Workspace, to name: String) {
        let b = ws.branch, m = ws.isMain
        Task {
            await Task.detached(priority: .userInitiated) { PomCore.shared.workspaceRename(branch: b, isMain: m, displayName: name) }.value
            await refreshWorkspaces()
        }
    }

    private var pollTask: Task<Void, Never>?

    private lazy var watcher = WorkspaceWatcher { [weak self] in
        Task { @MainActor in
            guard let self, self.appActive else { return }
            await self.refreshWorkspaces()
        }
    }
    private var powerMult: UInt64 { ProcessInfo.processInfo.isLowPowerModeEnabled ? 2 : 1 }

    private var booted = false
    func boot() {
        guard !booted else { return }
        guard let cfg = PomCore.resolveConfigPath() else {
            needsProject = true
            return
        }
        booted = true
        needsProject = false
        loading = true
        bootDo(cfg)
    }

    private var lastProjectFile: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/state/pom/last_project")
    }
    private func setLastProject(_ dir: String) {
        let f = lastProjectFile
        try? FileManager.default.createDirectory(at: f.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? dir.write(to: f, atomically: true, encoding: .utf8)
    }

    func bootProject(_ dir: String) { setLastProject(dir); booted = false; boot() }

    func openExistingSession() {
        let p = NSOpenPanel()
        p.canChooseDirectories = true; p.canChooseFiles = false; p.allowsMultipleSelection = false
        p.prompt = "Open"; p.message = "Choose your session folder (the one with pom.yml)"
        guard p.runModal() == .OK, let url = p.url else { return }
        let dir = url.path
        let hasCfg = ["pom.yml"].contains {
            FileManager.default.fileExists(atPath: (dir as NSString).appendingPathComponent($0))
        }
        guard hasCfg else {
            openError = "No pom.yml in “\(url.lastPathComponent)”. Pick the session root (the folder that has pom.yml), or start a new session."
            return
        }
        bootProject(dir)
    }

    private func bootDo(_ cfg: String) {
        Task {
            await Task.detached(priority: .userInitiated) { PomCore.shared.start(configPath: cfg) }.value
            if let err = PomCore.shared.initError { bootError = err; loading = false; return }
            installGlobalKeys()
            observeActivity()
            Notifier.requestAuth()
            Notifier.onOpenWorkspace = { [weak self] wsKey in
                guard let self else { return }
                self.selection = wsKey
                if !wsKey.hasPrefix("main:") { self.uiStore?.state(for: wsKey).pane = .claude }
            }
            await refresh()
            loading = false
            await refreshConfigHealth()
            if workspaces.isEmpty {
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                await refresh()
            }
            startPolling()
            Task { await loadSessions() }
            startPRPolling()
        }
    }

    private var prPollTask: Task<Void, Never>?
    private func startPRPolling() {
        prPollTask?.cancel()
        prPollTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            for _ in 0..<8 {
                guard let self, !Task.isCancelled else { return }
                if self.appActive { await self.refreshPRs(); await self.refreshJira() }
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
            var interval: UInt64 = 45
            while !Task.isCancelled {
                guard let self else { return }
                if self.appActive {
                    let changed = await self.refreshPRs()
                    await self.refreshJira()
                    interval = changed ? 45 : min(interval * 2, 240)
                }
                try? await Task.sleep(nanoseconds: interval * 1_000_000_000 * self.powerMult)
            }
        }
    }

    @discardableResult
    func refreshPRs() async -> Bool { await prsvm.refresh() }

    func refreshJira() async { await jiravm.refresh(branches: workspaces.map(\.branch)) }

    private func observeActivity() {
        let nc = NotificationCenter.default
        nc.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.appActive = true; await self?.refresh() }
        }
        nc.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.appActive = false }
        }
        nc.addObserver(forName: NSApplication.willTerminateNotification, object: nil, queue: .main) { _ in
            PomCore.shared.ptyReap()
        }
        nc.addObserver(forName: NSWindow.didEnterFullScreenNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.fullscreen = true }
        }
        nc.addObserver(forName: NSWindow.didExitFullScreenNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.fullscreen = false }
        }
    }

    func loadSessions() async { await sessionsvm.load() }

    func refresh() async {
        await refreshWorkspaces()
        await refreshAgents()
    }

    func refreshWorkspaces() async {
        if workspaces.isEmpty, let shallow = await fetchWorkspaces(git: false) {
            workspaces = shallow
            if selection == nil { selection = shallow.first?.id }
        }
        guard let full = await fetchWorkspaces(git: true) else { return }
        if full != workspaces { workspaces = full }
        if selection == nil { selection = full.first?.id }
        if let first = full.first { watcher.start(root: (first.path as NSString).deletingLastPathComponent) }
    }

    private func fetchWorkspaces(git: Bool) async -> [Workspace]? {
        await wsvm.fetch(git: git)
    }

    func refreshAgents() async {
        await agentsvm.refresh(notify: notifyClaude, whenFocused: SoundPrefs.shared.whenFocused,
                               activeSelection: selection, appActive: appActive) { [weak self] title, event, ws in
            guard let self else { return }
            if event != "working" { Notifier.notify(title: title, body: self.workspaceTitle(ws), wsKey: ws) }
            SoundPrefs.shared.fire(source: "claude", event: event)
        }
    }

    func refreshLiveness() async { await wsvm.refreshLiveness() }

    @Published var prPeek: String?
    private var prPeekClear: DispatchWorkItem?
    func prPeekEnter(_ id: String) { prPeekClear?.cancel(); if prPeek != id { prPeek = id } }
    func prPeekLeave() {
        prPeekClear?.cancel()
        let w = DispatchWorkItem { [weak self] in self?.prPeek = nil }
        prPeekClear = w
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: w)
    }

    func stopAllServices(_ ws: Workspace) {
        Task {
            for repo in ws.repos {
                for svc in (repo.services ?? []) where svc.running {
                    let ref: [String: Any] = ["branch": ws.branch, "is_main": ws.isMain, "repo": repo.name, "svc": svc.name]
                    let body = (try? JSONSerialization.data(withJSONObject: ref)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
                    _ = await Task.detached { PomCore.shared.serviceControl(refJSON: body, action: "stop") }.value
                }
            }
            await refreshWorkspaces()
        }
    }

    private func workspaceTitle(_ wsKey: String) -> String {
        workspaces.first { $0.id == wsKey }?.title ?? wsKey
    }

    var selectedWorkspace: Workspace? {
        workspaces.first { $0.id == selection }
    }

    private var agentTask: Task<Void, Never>?
    private func startPolling() {
        pollTask?.cancel(); agentTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                try? await Task.sleep(nanoseconds: 300_000_000_000 * self.powerMult)
                if self.appActive { await self.refreshWorkspaces() }
            }
        }
        agentTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 3_000_000_000)
                guard let self else { return }
                await self.refreshAgents()
                if self.appActive { await self.refreshLiveness() }
            }
        }
    }
}
