import SwiftUI
import UniformTypeIdentifiers

struct ServicesBoard: View {
    @Environment(AppState.self) var state
    @EnvironmentObject var theme: ThemeManager
    let workspace: Workspace
    var openPane: (PaneKind) -> Void = { _ in }
    var openTerminal: (String) -> Void = { _ in }
    var onPrepareMain: () -> Void = {}
    @StateObject private var peek = PeekStore()
    @State private var investigating = false
    @State private var dragging: String?
    private let colSpacing: CGFloat = 16

    private func createInvestigate() {
        investigating = true
        let df = DateFormatter(); df.dateFormat = "MMdd"
        let alphabet = Array("abcdefghijklmnopqrstuvwxyz0123456789")
        let rand = String((0..<4).map { _ in alphabet.randomElement()! })
        let branch = "investigate-\(df.string(from: Date()))-\(rand)"
        let repos = workspace.repos.map(\.name)
        state.startCreate(branch: branch, repos: repos, displayName: "")
        investigating = false
    }

    private var runningWindows: [String] {
        workspace.repos.flatMap { $0.services ?? [] }
            .filter { $0.running }.compactMap { $0.tmuxWindow }
    }

    var body: some View {
        VStack(spacing: 0) {
            if workspace.isMain { goldenSourceBar }
            gridBoard
        }
        .background(Theme.bg)
        .environmentObject(peek)
        .onAppear { peek.isActive = { [weak state] in state?.appActive ?? true }; peek.sync(windows: runningWindows) }
        .onChange(of: runningWindows) { peek.sync(windows: runningWindows) }
        .onDisappear { peek.stop() }
    }

    private var gridBoard: some View {
        let repos = state.orderedRepos(workspace.repos)
        let names = repos.map(\.name)
        return ScrollView(.vertical) {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 360, maximum: 560), spacing: colSpacing, alignment: .top)],
                      alignment: .center, spacing: 16) {
                ForEach(repos, id: \.id) { repo in
                    RepoColumn(repo: repo, branch: workspace.branch, isMain: workspace.isMain, openPane: openPane, openTerminal: openTerminal)
                        .frame(maxWidth: .infinity, alignment: .top)
                        .opacity(dragging == repo.name ? 0.35 : 1)
                        .onDrag {
                            dragging = repo.name
                            return NSItemProvider(object: repo.name as NSString)
                        }
                        .onDrop(of: [UTType.text], delegate: RepoReorderDelegate(target: repo.name, names: names, dragging: $dragging) {
                            state.moveRepo($0, toIndex: $1, in: $2)
                        })
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .animation(.spring(response: 0.3, dampingFraction: 0.85), value: names)
        }
        .onDrop(of: [UTType.text], isTargeted: nil) { _ in dragging = nil; return true }
    }

    private var goldenSourceBar: some View {
        HStack(spacing: 8) {
            Text("GOLDEN SOURCE").font(.system(size: 10.5, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
            Spacer()
            Button(action: createInvestigate) {
                HStack(spacing: 5) {
                    Image(systemName: investigating ? "hourglass" : "plus.viewfinder").font(.system(size: 11))
                    Text(investigating ? "Creating…" : "Scratch workspace").font(.system(size: 12, weight: .medium))
                }
                .foregroundStyle(Theme.fgMuted)
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(Theme.chip, in: Capsule())
                .overlay(Capsule().strokeBorder(Theme.chipBd))
            }
            .buttonStyle(.plain).disabled(investigating)
            .help("Create a throwaway investigate-<date> workspace (all main repos) to reproduce a bug in isolation — no ticket/branch to name.")
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(Theme.bgSoft)
        .overlay(Rectangle().fill(Theme.borderSoft).frame(height: 1), alignment: .bottom)
    }
}

struct RepoColumn: View {
    @Environment(AppState.self) var state
    @EnvironmentObject var theme: ThemeManager
    let repo: Repo
    let branch: String
    let isMain: Bool
    var openPane: (PaneKind) -> Void = { _ in }
    var openTerminal: (String) -> Void = { _ in }

    private var services: [Service] { repo.services ?? [] }
    private var running: Int { services.filter(\.running).count }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text(repo.alias ?? repo.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.fg)
                Text("\(running)/\(services.count)").font(Theme.mono(11)).foregroundStyle(Theme.dim)
                if repo.dirty > 0 {
                    Text("\(repo.dirty) dirty").font(Theme.mono(10.5, .semibold)).foregroundStyle(Theme.warn)
                }
                Spacer(minLength: 0)
                if running < services.count {
                    IconButton("play", tip: "Start all") { runAll("start") }
                }
                if running > 0 {
                    IconButton("arrow.clockwise", tip: "Restart all") { runAll("restart") }
                    IconButton("stop", tip: "Stop all") { runAll("stop") }
                }
                IconButton("terminal", tip: "Open terminal in \(repo.alias ?? repo.name)") { openRepoTerminal() }
                if services.contains(where: { ($0.port ?? 0) > 0 }) { portMenu }
                if let scs = repo.shortcuts, !scs.isEmpty { shortcutMenu(scs) }
                repoMenu
            }
            .padding(.horizontal, 4)

            if services.isEmpty {
                Text("no services").font(.system(size: 12)).foregroundStyle(Theme.dim).padding(.leading, 4)
            } else {
                ForEach(services) { svc in
                    SvcCard(service: svc, branch: branch, isMain: isMain, repoName: repo.name, openTerminal: openTerminal)
                }
            }
        }
        .frame(width: 360, alignment: .top)
    }

    private func shortcutMenu(_ scs: [Shortcut]) -> some View {
        PopoverMenu(systemImage: "bolt") { close in
            ForEach(scs) { sc in
                PopItem(sc.desc.isEmpty ? sc.cmd : sc.desc, icon: "bolt") { runShortcut(sc); close() }
            }
        }
        .help("Shortcuts — \(scs.count) command\(scs.count == 1 ? "" : "s")")
    }

    private var portMenu: some View {
        PopoverMenu(systemImage: "number") { close in
            ForEach(services.filter { ($0.port ?? 0) > 0 }) { svc in
                PopItem("\(svc.name)   :\(svc.port ?? 0)", icon: "doc.on.doc") {
                    copyPort("127.0.0.1:\(svc.port ?? 0)"); close()
                }
            }
        }
        .help("Copy service port")
    }

    private func copyPort(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    private var repoMenu: some View {
        IconButton("square.and.pencil", tip: "Open in editor") { openEditor() }
    }

    private func openEditor() {
        let b = branch, m = isMain, rp = repo.name, ed = state.editorPref
        ServicesStore.openEditor(branch: b, isMain: m, repo: rp, editor: ed)
    }

    private func openRepoTerminal() {
        let b = branch, m = isMain, rp = repo.name
        Task {
            let holder = await Task.detached(priority: .userInitiated) { () -> String? in
                struct R: Decodable { var window: String? }
                let d = ServicesStore.shortcutRun(branch: b, isMain: m, repo: rp, cmd: "exec zsh")
                return PomJSON.decode(R.self, from: d)?.window
            }.value
            if let holder, !holder.isEmpty { openTerminal(holder) }
        }
    }

    private func runShortcut(_ sc: Shortcut) {
        let b = branch, m = isMain, rp = repo.name, c = sc.cmd
        Task {
            let holder = await Task.detached(priority: .userInitiated) { () -> String? in
                struct R: Decodable { var window: String? }
                let d = ServicesStore.shortcutRun(branch: b, isMain: m, repo: rp, cmd: c)
                return PomJSON.decode(R.self, from: d)?.window
            }.value
            if let holder, !holder.isEmpty { openTerminal(holder) }
            await state.refresh()
        }
    }

    private func runAll(_ action: String) {
        let targets = ServiceRun.targets(services, action: action)
        guard !targets.isEmpty else { return }
        let keys = targets.map { state.svcKey(branch: branch, repo: repo.name, svc: $0.name) }
        let label = action == "stop" ? "stopping…" : (action == "restart" ? "restarting…" : "starting…")
        withAnimation(.easeInOut(duration: 0.2)) { for k in keys { state.pendingSvc[k] = label } }
        Task {
            await withTaskGroup(of: Void.self) { group in
                for svc in targets {
                    let ref: [String: Any] = ["branch": branch, "is_main": isMain, "repo": repo.name, "svc": svc.name]
                    let body = (try? JSONSerialization.data(withJSONObject: ref)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
                    group.addTask { _ = await ServicesStore.control(refJSON: body, action: action) }
                }
            }
            await state.refresh()
            withAnimation(.easeInOut(duration: 0.25)) { for k in keys { state.pendingSvc[k] = nil } }
        }
    }
}

struct SvcCard: View {
    @Environment(AppState.self) var state
    @EnvironmentObject var peek: PeekStore
    @EnvironmentObject var theme: ThemeManager
    let service: Service
    let branch: String
    let isMain: Bool
    let repoName: String
    var openTerminal: (String) -> Void = { _ in }
    @State private var busy = false
    @State private var busyLabel = "starting…"
    @State private var envPick: String?
    @State private var startError: String?
    @State private var showLogModal = false

    private var curEnv: String { envPick ?? service.env ?? "local" }
    private var envIsRemote: Bool { curEnv != "local" }
    private var myKey: String { state.svcKey(branch: branch, repo: repoName, svc: service.name) }
    private var pendingLabel: String? { state.pendingSvc[myKey] }
    private var isBusy: Bool { busy || pendingLabel != nil }

    private var crashMsg: String? { (service.crashed == true) ? (service.crashLog ?? "") : nil }
    private var errText: String? {
        if let e = startError, !e.isEmpty { return e }
        if let c = crashMsg, !c.isEmpty { return c }
        return nil
    }
    private var expands: Bool {
        if service.running, let win = service.tmuxWindow, let l = peek.lines[win], !l.isEmpty { return true }
        return !service.running && errText != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            headRow
            if expands {
                Divider().overlay(Theme.borderSoft)
                expansion
                    .clipShape(UnevenRoundedRectangle(bottomLeadingRadius: 10, bottomTrailingRadius: 10))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(service.running ? Theme.ok.opacity(0.3) : Theme.borderSoft))
        .animation(.easeInOut(duration: 0.22), value: service.running)
        .animation(.easeInOut(duration: 0.18), value: isBusy)
    }

    private var headRow: some View {
        HStack(spacing: 9) {
            Circle().fill(service.running ? Theme.ok : (service.crashed == true ? Theme.danger : Theme.dim)).frame(width: 7, height: 7).fixedSize()
            Text(service.name).font(Theme.mono(13, .medium)).foregroundStyle(Theme.fg)
                .lineLimit(1).truncationMode(.tail).layoutPriority(1)
            statusLabel.fixedSize()
            Spacer(minLength: 6)
            if let modes = service.modes, modes.count > 0 {
                ChipSelect(text: service.mode ?? "mode", color: Theme.wsAccent, options: modes, current: service.mode) { setMode($0) }
            }
            if let profiles = service.profiles, profiles.count > 1 {
                ChipSelect(text: curEnv, color: envIsRemote ? Theme.warn : Theme.ok, options: profiles, current: curEnv) { setEnv($0) }
            } else if envIsRemote {
                chipStatic(service.env ?? "", color: Theme.warn)
            }
            rightControls
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .contentShape(Rectangle())
        .onTapGesture { if service.running, let win = service.tmuxWindow { openTerminal(win) } }
    }

    @ViewBuilder private var statusLabel: some View {
        if isBusy {
            EmptyView()
        } else if service.running {
            EmptyView()
        } else {
            Text(crashMsg != nil ? "crashed" : "stopped")
                .font(Theme.mono(9.5)).textCase(.uppercase).kerning(0.5)
                .foregroundStyle(crashMsg != nil ? Theme.danger : Theme.dim)
        }
    }

    @ViewBuilder private var rightControls: some View {
        if isBusy {
            HStack(spacing: 6) {
                Spinner(size: 12)
                Text(busy ? busyLabel : (pendingLabel ?? "…")).font(Theme.mono(10.5)).foregroundStyle(Theme.accent).lineLimit(1)
            }
        } else if service.running {
            IconButton("arrow.clockwise", tip: "Restart") { act("restart") }
            if (service.port ?? 0) > 0 || peekURL != nil {
                IconButton("arrow.up.forward.app", tip: "Open in browser") { openInBrowser() }
            }
            IconButton("stop.fill", tip: "Stop") { act("stop") }
        } else {
            Button { act("start") } label: {
                Image(systemName: "play.fill").font(.system(size: 10))
                    .foregroundStyle(Theme.ok)
                    .frame(width: 26, height: 22)
                    .background(Theme.ok.opacity(0.14), in: RoundedRectangle(cornerRadius: 6))
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.ok.opacity(0.25)))
            }
            .buttonStyle(.plain).fixedSize()
            .help("Start")
        }
    }

    @ViewBuilder private var expansion: some View {
        if service.running, let win = service.tmuxWindow, let lines = peek.lines[win], !lines.isEmpty {
            SvcPeekView(lines: lines).onTapGesture { openTerminal(win) }.help("Open terminal / view log")
        } else if let err = errText {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .top, spacing: 6) {
                    Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 9)).foregroundStyle(Theme.danger)
                    Text(err).font(Theme.mono(10)).foregroundStyle(Theme.danger)
                        .lineLimit(4).truncationMode(.tail).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
                .onTapGesture { showLogModal = true }
                HStack(spacing: 6) {
                    Button { showLogModal = true } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "doc.plaintext").font(.system(size: 9))
                            Text("View log").font(.system(size: 10.5, weight: .semibold))
                        }
                        .foregroundStyle(Theme.fgMuted)
                        .padding(.horizontal, 8).padding(.vertical, 2)
                        .background(Theme.chip, in: RoundedRectangle(cornerRadius: 5))
                    }
                    .buttonStyle(.plain)
                    if err.contains("port") {
                        Button { act("start", relocate: true) } label: {
                            Text("Use a new port").font(.system(size: 10.5, weight: .semibold))
                                .foregroundStyle(Theme.accent)
                                .padding(.horizontal, 8).padding(.vertical, 2)
                                .background(Theme.accent.opacity(0.15), in: RoundedRectangle(cornerRadius: 5))
                        }
                        .buttonStyle(.plain)
                    }
                    Button { fixingThis ? state.reopenAgent() : fixWithClaude(err) } label: {
                        HStack(spacing: 4) {
                            if fixingThis { Spinner(size: 10) }
                            else { Image(systemName: "sparkles").font(.system(size: 9)) }
                            Text(fixingThis ? "Fixing…" : "Fix with Claude").font(.system(size: 10.5, weight: .semibold))
                        }
                        .foregroundStyle(Theme.wsAccent)
                        .padding(.horizontal, 8).padding(.vertical, 2)
                        .background(Theme.wsAccent.opacity(0.15), in: RoundedRectangle(cornerRadius: 5))
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 9)
            .frame(maxWidth: .infinity, alignment: .leading)
            .help("Click to view the full log")
            .sheet(isPresented: $showLogModal) { CrashLogSheet(title: "\(repoName) · \(service.name)", log: err) }
        }
    }

    private var fixTarget: String { "\(branch)|\(repoName)|\(service.name)" }
    private var fixingThis: Bool { state.agentModel != nil && state.agentTarget == fixTarget }

    private func fixWithClaude(_ err: String) {
        let prompt = """
        The "\(service.name)" service in repo "\(repoName)" won't start. Its last output was:

        \(err)

        Diagnose why it died and fix the root cause (setup / environment / config / \
        ports / database), then restart it via service_restart and confirm with \
        services/service_logs that it actually comes up. Fix it end to end — don't \
        hand back a to-do list.
        """
        state.launchAgent(
            title: "Fix \(service.name)",
            subtitle: "Pomelo's Doctor reads the logs + config, fixes the cause, restarts, and verifies.",
            runningLabel: "Fixing \(service.name)…",
            branch: branch, isMain: isMain, role: "fixer", firstTurn: prompt, target: fixTarget)
    }

    private func chipStatic(_ text: String, color: Color) -> some View {
        Text(text).font(Theme.mono(11)).foregroundStyle(color)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(Theme.chip, in: Capsule())
            .overlay(Capsule().strokeBorder(color.opacity(0.4)))
    }

    private func setMode(_ mode: String) {
        let r = repoName, sv = service.name
        ServicesStore.serviceMode(repo: r, svc: sv, mode: mode)
    }

    private func setEnv(_ env: String) {
        envPick = env
        let wasRunning = service.running
        busy = true
        busyLabel = wasRunning ? "switching to \(env)…" : "saving \(env)…"
        let b = branch, m = isMain, r = repoName, sv = service.name
        Task {
            await ServicesStore.envSet(branch: b, isMain: m, repo: r, svc: sv, env: env)
            if wasRunning {
                let ref: [String: Any] = ["branch": b, "is_main": m, "repo": r, "svc": sv]
                let body = (try? JSONSerialization.data(withJSONObject: ref)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
                _ = await ServicesStore.control(refJSON: body, action: "restart")
            }
            try? await Task.sleep(nanoseconds: 300_000_000)
            await state.refreshWorkspaces()
            envPick = nil
            busy = false
        }
    }

    private func act(_ action: String, relocate: Bool = false) {
        busy = true
        busyLabel = action == "stop" ? "stopping…" : (action == "restart" ? "restarting…" : "starting…")
        startError = nil
        var ref: [String: Any] = ["branch": branch, "is_main": isMain, "repo": repoName, "svc": service.name]
        if relocate { ref["relocate"] = true }
        if action == "start" || action == "restart" { ref["auto_relocate"] = true }
        let body = (try? JSONSerialization.data(withJSONObject: ref)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        Task {
            let data = await ServicesStore.control(refJSON: body, action: action)
            if let msg = SvcCard.actionError(data) { startError = msg; busy = false; return }
            try? await Task.sleep(nanoseconds: 300_000_000)
            await state.refreshWorkspaces()
            busy = false
        }
    }

    static func actionError(_ data: Data) -> String? {
        if let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any], obj["ok"] as? Bool == true {
            return nil
        }
        let s = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return s.isEmpty ? "unknown error" : s
    }

    private var peekURL: String? {
        guard let win = service.tmuxWindow, let lines = peek.lines[win] else { return nil }
        let re = try? NSRegularExpression(pattern: "https?://(?:localhost|127\\.0\\.0\\.1|0\\.0\\.0\\.0):(\\d{2,5})")
        for l in lines.reversed() {
            let ns = l as NSString
            if let m = re?.firstMatch(in: l, range: NSRange(location: 0, length: ns.length)), m.numberOfRanges > 1 {
                return "http://localhost:" + ns.substring(with: m.range(at: 1))
            }
        }
        return nil
    }

    private func openInBrowser() {
        if (service.port ?? 0) > 0 {
            let b = branch, r = repoName, sv = service.name
            Task {
                let url = await Task.detached(priority: .userInitiated) { () -> String? in
                    struct R: Decodable { var url = "" }
                    let d = ServicesStore.serviceURL(branch: b, repo: r, svc: sv)
                    return PomJSON.decode(R.self, from: d)?.url
                }.value
                if let url, let u = URL(string: url) { NSWorkspace.shared.open(u) }
            }
        } else if let url = peekURL, let u = URL(string: url) {
            NSWorkspace.shared.open(u)
        }
    }
    private func copyToPasteboard(_ s: String) {
        NSPasteboard.general.clearContents(); NSPasteboard.general.setString(s, forType: .string)
    }
}

struct CrashLogSheet: View {
    let title: String
    let log: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 11)).foregroundStyle(Theme.danger)
                Text(title).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.fg).lineLimit(1)
                Spacer()
                Button { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(log, forType: .string) } label: {
                    Label("Copy", systemImage: "doc.on.doc").font(.system(size: 11))
                }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
                Button { dismiss() } label: { Image(systemName: "xmark").font(.system(size: 12)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            Divider().overlay(Theme.borderSoft)
            ScrollView(.vertical) {
                Text(log.isEmpty ? "(no output)" : log).font(Theme.mono(11.5)).foregroundStyle(Theme.fg)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(14).frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Theme.bg)
        }
        .frame(width: 760, height: 520).background(Theme.bgSoft)
    }
}

struct SvcPeekView: View {
    let lines: [String]
    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            ForEach(Array(lines.enumerated()), id: \.offset) { _, l in
                Text(l.isEmpty ? " " : l).font(Theme.mono(10.5)).foregroundStyle(Theme.fgMuted)
                    .lineLimit(1).truncationMode(.tail)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 10).padding(.vertical, 8)
        .background(Theme.bg)
    }
}

struct RepoReorderDelegate: DropDelegate {
    let target: String
    let names: [String]
    @Binding var dragging: String?
    let move: (String, Int, [String]) -> Void

    func dropUpdated(info: DropInfo) -> DropProposal? { DropProposal(operation: .move) }
    func dropEntered(info: DropInfo) {
        guard let d = dragging, d != target, let to = names.firstIndex(of: target) else { return }
        withAnimation(.spring(response: 0.3, dampingFraction: 0.85)) { move(d, to, names) }
    }
    func performDrop(info: DropInfo) -> Bool { dragging = nil; return true }
}
