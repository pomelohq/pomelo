import SwiftUI

struct ServicesBoard: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    let workspace: Workspace
    var openPane: (PaneKind) -> Void = { _ in }
    var openTerminal: (String) -> Void = { _ in }
    var onPrepareMain: () -> Void = {}
    @StateObject private var peek = PeekStore()
    @State private var dropTarget: String?
    @State private var investigating = false

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
            ScrollView(.horizontal) {
                let ordered = state.orderedRepos(workspace.repos)
                HStack(alignment: .top, spacing: 16) {
                    ForEach(ordered) { repo in
                        HStack(alignment: .top, spacing: 0) {
                            RoundedRectangle(cornerRadius: 2)
                                .fill(dropTarget == repo.name ? Theme.accent : Color.clear)
                                .frame(width: 3)
                                .padding(.trailing, 8)
                            RepoColumn(repo: repo, branch: workspace.branch, isMain: workspace.isMain, openPane: openPane, openTerminal: openTerminal)
                        }
                        .draggable(repo.name) { columnDragPreview(repo) }
                        .dropDestination(for: String.self) { items, _ in
                            dropTarget = nil
                            guard let dragged = items.first else { return false }
                            state.moveRepo(dragged, before: repo.name, in: ordered.map(\.name))
                            return true
                        } isTargeted: { hovering in
                            dropTarget = hovering ? repo.name : (dropTarget == repo.name ? nil : dropTarget)
                        }
                    }
                }
                .padding(.horizontal, 14).padding(.vertical, 16)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
        .background(Theme.bg)
        .environmentObject(peek)
        .onAppear { peek.isActive = { [weak state] in state?.appActive ?? true }; peek.sync(windows: runningWindows) }
        .onChange(of: runningWindows) { peek.sync(windows: runningWindows) }
        .onDisappear { peek.stop() }
    }

    private func columnDragPreview(_ repo: Repo) -> some View {
        let svcs = repo.services ?? []
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text(repo.alias ?? repo.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.fg)
                Text("\(svcs.filter(\.running).count)/\(svcs.count)").font(Theme.mono(11)).foregroundStyle(Theme.dim)
            }
            ForEach(svcs.prefix(8)) { svc in
                HStack(spacing: 8) {
                    Circle().fill(svc.running ? Theme.ok : Theme.dim).frame(width: 7, height: 7)
                    Text(svc.name).font(Theme.mono(12)).foregroundStyle(Theme.fgMuted)
                    Spacer(minLength: 0)
                }
            }
        }
        .padding(12)
        .frame(width: 360, alignment: .leading)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.accent))
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
            Button(action: onPrepareMain) {
                HStack(spacing: 5) {
                    Image(systemName: "arrow.triangle.2.circlepath").font(.system(size: 11))
                    Text("Prepare main").font(.system(size: 12, weight: .medium))
                }
                .foregroundStyle(Theme.accent)
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(Theme.accentSoft, in: Capsule())
            }
            .buttonStyle(.plain)
            .help("Reset databases + migrate + seed. New workspaces clone these DBs via TEMPLATE.")
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(Theme.bgSoft)
        .overlay(Rectangle().fill(Theme.borderSoft).frame(height: 1), alignment: .bottom)
    }
}

struct RepoColumn: View {
    @EnvironmentObject var state: AppState
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
        Menu {
            ForEach(scs) { sc in
                Button {
                    runShortcut(sc)
                } label: {
                    Text(sc.desc.isEmpty ? sc.cmd : sc.desc)
                    if !sc.desc.isEmpty { Text(sc.cmd) }
                }
            }
        } label: {
            Image(systemName: "bolt.fill").font(.system(size: 11)).foregroundStyle(Theme.accent)
                .frame(width: 24, height: 22)
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
        .help("Shortcuts — \(scs.count) command\(scs.count == 1 ? "" : "s")")
    }

    private var repoMenu: some View {
        Menu {
            if running < services.count { Button("Start all") { runAll("start") } }
            if running > 0 {
                Button("Stop all") { runAll("stop") }
                Button("Restart all") { runAll("restart") }
            }
            Divider()
            Button("Open in editor") { openEditor() }
        } label: {
            Image(systemName: "ellipsis").font(.system(size: 12)).foregroundStyle(Theme.dim)
                .frame(width: 24, height: 22)
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
    }

    private func openEditor() {
        let b = branch, m = isMain, rp = repo.name, ed = state.editorPref
        Task.detached(priority: .userInitiated) { PomCore.shared.openEditor(branch: b, isMain: m, repo: rp, editor: ed, resolveOnly: false) }
    }

    private func openRepoTerminal() {
        let b = branch, m = isMain, rp = repo.name
        Task {
            let holder = await Task.detached(priority: .userInitiated) { () -> String? in
                struct R: Decodable { var window: String? }
                let d = PomCore.shared.shortcutRun(branch: b, isMain: m, repo: rp, cmd: "exec zsh")
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
                let d = PomCore.shared.shortcutRun(branch: b, isMain: m, repo: rp, cmd: c)
                return PomJSON.decode(R.self, from: d)?.window
            }.value
            if let holder, !holder.isEmpty { openTerminal(holder) }
            await state.refresh()
        }
    }

    private func runAll(_ action: String) {
        let targets = action == "restart" ? services
            : services.filter { action == "start" ? !$0.running : $0.running }
        guard !targets.isEmpty else { return }
        let keys = targets.map { state.svcKey(branch: branch, repo: repo.name, svc: $0.name) }
        let label = action == "stop" ? "stopping…" : (action == "restart" ? "restarting…" : "starting…")
        withAnimation(.easeInOut(duration: 0.2)) { for k in keys { state.pendingSvc[k] = label } }
        Task {
            await withTaskGroup(of: Void.self) { group in
                for svc in targets {
                    let ref: [String: Any] = ["branch": branch, "is_main": isMain, "repo": repo.name, "svc": svc.name]
                    let body = (try? JSONSerialization.data(withJSONObject: ref)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
                    group.addTask { _ = await Task.detached(priority: .userInitiated) { PomCore.shared.serviceControl(refJSON: body, action: action) }.value }
                }
            }
            await state.refresh()
            withAnimation(.easeInOut(duration: 0.25)) { for k in keys { state.pendingSvc[k] = nil } }
        }
    }
}

struct IconButton: View {
    let systemName: String
    let tip: String
    let action: () -> Void
    @State private var hover = false
    init(_ systemName: String, tip: String, action: @escaping () -> Void) {
        self.systemName = systemName; self.tip = tip; self.action = action
    }
    var body: some View {
        Button(action: action) {
            Image(systemName: systemName).font(.system(size: 12))
                .foregroundStyle(hover ? Theme.fg : Theme.dim)
                .frame(width: 24, height: 22)
                .background(hover ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 5))
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help(tip)
    }
}

struct SvcCard: View {
    @EnvironmentObject var state: AppState
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

    private var curEnv: String { envPick ?? service.env ?? "local" }
    private var envIsRemote: Bool { curEnv != "local" }
    private var myKey: String { state.svcKey(branch: branch, repo: repoName, svc: service.name) }
    private var pendingLabel: String? { state.pendingSvc[myKey] }
    private var isBusy: Bool { busy || pendingLabel != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            head
            Divider().overlay(Theme.borderSoft)
            body(for: service)
                .clipShape(UnevenRoundedRectangle(bottomLeadingRadius: 12, bottomTrailingRadius: 12))
        }
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
        .overlay(alignment: .leading) {
            if service.running {
                RoundedRectangle(cornerRadius: 1).fill(Theme.ok).frame(width: 2).padding(.vertical, 8)
            }
        }
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.borderSoft))
        .animation(.easeInOut(duration: 0.22), value: service.running)
        .animation(.easeInOut(duration: 0.18), value: isBusy)
    }

    private var head: some View {
        HStack(spacing: 9) {
            Circle().fill(service.running ? Theme.ok : (service.crashed == true ? Theme.danger : Theme.dim)).frame(width: 8, height: 8)
                .help(service.running ? (service.port.map { "port :\($0)" } ?? "running") : (service.crashed == true ? "crashed" : "stopped"))
            Text(service.name).font(Theme.mono(13, .medium)).foregroundStyle(Theme.fg).lineLimit(1)
            Spacer(minLength: 6)
            if let modes = service.modes, modes.count > 0 {
                ChipSelect(text: service.mode ?? "mode", color: Theme.wsAccent, options: modes,
                           current: service.mode) { setMode($0) }
            }
            if let profiles = service.profiles, profiles.count > 1 {
                ChipSelect(text: curEnv, color: envIsRemote ? Theme.warn : Theme.ok,
                           options: profiles, current: curEnv) { setEnv($0) }
            } else if envIsRemote {
                chipStatic(service.env ?? "", color: Theme.warn)
            }
            if service.running {
                IconButton("arrow.clockwise", tip: "Restart") { act("restart") }
                if (service.port ?? 0) > 0 || peekURL != nil {
                    IconButton("arrow.up.forward.app", tip: "Open in browser") { openInBrowser() }
                }
                IconButton("stop.fill", tip: "Stop") { act("stop") }
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 11)
    }

    @ViewBuilder private func body(for svc: Service) -> some View {
        if isBusy {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small).scaleEffect(0.7)
                Text(busy ? busyLabel : (pendingLabel ?? "…")).font(.system(size: 11.5)).foregroundStyle(Theme.accent)
                Spacer()
            }
            .padding(.horizontal, 14).padding(.vertical, 9)
            .transition(.opacity)
        } else if svc.running {
            if let win = svc.tmuxWindow, let lines = peek.lines[win], !lines.isEmpty {
                SvcPeekView(lines: lines).onTapGesture { openTerminal(win) }
                    .help("Open terminal / view log")
            } else {
                HStack(spacing: 6) {
                    Text("running").font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
                    if let p = svc.port, p > 0 {
                        Text(":\(p)").font(Theme.mono(11)).foregroundStyle(Theme.dim)
                    }
                    Spacer()
                }
                .padding(.horizontal, 14).padding(.vertical, 9)
                .contentShape(Rectangle())
                .onTapGesture { if let win = svc.tmuxWindow { openTerminal(win) } }
                .help("Open terminal / view log")
            }
        } else {
            let crashMsg = (service.crashed == true) ? (service.crashLog ?? "") : nil
            VStack(spacing: 0) {
                if let err = startError ?? crashMsg, !err.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(alignment: .top, spacing: 6) {
                            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 9)).foregroundStyle(Theme.danger)
                            Text(err).font(Theme.mono(10)).foregroundStyle(Theme.danger).lineLimit(6).textSelection(.enabled)
                            Spacer(minLength: 0)
                        }
                        HStack(spacing: 6) {
                            if err.contains("port") {
                                Button { act("start", relocate: true) } label: {
                                    Text("Use a new port").font(.system(size: 10.5, weight: .semibold))
                                        .foregroundStyle(Theme.accent)
                                        .padding(.horizontal, 8).padding(.vertical, 2)
                                        .background(Theme.accent.opacity(0.15), in: Capsule())
                                }
                                .buttonStyle(.plain)
                            }
                            Button { fixingThis ? state.reopenAgent() : fixWithClaude(err) } label: {
                                HStack(spacing: 4) {
                                    if fixingThis { ProgressView().controlSize(.small).scaleEffect(0.5) }
                                    else { Image(systemName: "sparkles").font(.system(size: 9)) }
                                    Text(fixingThis ? "Fixing…" : "Fix with Claude").font(.system(size: 10.5, weight: .semibold))
                                }
                                .foregroundStyle(Theme.wsAccent)
                                .padding(.horizontal, 8).padding(.vertical, 2)
                                .background(Theme.wsAccent.opacity(0.15), in: Capsule())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 9)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.danger.opacity(0.10), in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.danger.opacity(0.22)))
                    .padding(.horizontal, 10).padding(.top, 10)
                    .help(err)
                }
                Button { act("start") } label: {
                    HStack {
                        Text(crashMsg != nil ? "crashed" : "stopped").font(.system(size: 11.5))
                            .foregroundStyle(crashMsg != nil ? Theme.danger : Theme.fgMuted)
                        Spacer()
                        HStack(spacing: 4) {
                            Image(systemName: "play.fill").font(.system(size: 9))
                            Text("start").font(.system(size: 11.5, weight: .semibold))
                        }
                        .foregroundStyle(Theme.ok)
                        .padding(.horizontal, 9).padding(.vertical, 2)
                        .background(Theme.ok.opacity(0.15), in: Capsule())
                    }
                    .padding(.horizontal, 14).padding(.vertical, 9)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
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
        Task.detached(priority: .userInitiated) { PomCore.shared.serviceMode(repo: r, svc: sv, mode: mode) }
    }

    private func setEnv(_ env: String) {
        envPick = env
        let wasRunning = service.running
        busy = true
        busyLabel = wasRunning ? "switching to \(env)…" : "saving \(env)…"
        let b = branch, m = isMain, r = repoName, sv = service.name
        Task {
            _ = await Task.detached { PomCore.shared.envSet(branch: b, isMain: m, repo: r, svc: sv, env: env) }.value
            if wasRunning {
                let ref: [String: Any] = ["branch": b, "is_main": m, "repo": r, "svc": sv]
                let body = (try? JSONSerialization.data(withJSONObject: ref)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
                _ = await Task.detached { PomCore.shared.serviceControl(refJSON: body, action: "restart") }.value
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
        if action == "start" || action == "restart" { ref["auto_relocate"] = state.autoPickPort }
        let body = (try? JSONSerialization.data(withJSONObject: ref)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        Task {
            let data = await Task.detached { PomCore.shared.serviceControl(refJSON: body, action: action) }.value
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
                    let d = PomCore.shared.serviceURL(branch: b, repo: r, svc: sv)
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
