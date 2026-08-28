import SwiftUI

struct AgentEntry: Identifiable {
    enum Kind { case text, tool, system, error }
    let id = UUID()
    let kind: Kind
    var text: String
}

@MainActor
final class AgentStreamModel: ObservableObject {
    @Published var entries: [AgentEntry] = []
    @Published var running = true
    @Published var started = false   // first real frame arrived → stop showing "starting…"
    private var id: Int32 = 0

    var tick: Int { entries.count &+ (entries.last?.text.count ?? 0) }

    func start(branch: String, isMain: Bool, role: String, firstTurn: String) {
        guard id == 0 else { return }
        guard !firstTurn.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            append(.error, "Empty prompt — nothing to run."); running = false; return
        }
        id = StreamManager.shared.openClaude(branch: branch, isMain: isMain, mode: "", model: "sonnet", role: role) { [weak self] kind, bytes in
            guard let self else { return }
            if kind == .close { Task { @MainActor in self.running = false }; return }
            guard kind == .json,
                  let obj = try? JSONSerialization.jsonObject(with: Data(bytes)) as? [String: Any] else { return }
            let k = obj["kind"] as? String ?? ""
            switch k {
            case "text":
                let t = obj["text"] as? String ?? ""
                if !t.isEmpty { Task { @MainActor in self.append(.text, t) } }
            case "tool_use":
                let name = Self.prettyTool(obj["tool"] as? String ?? "tool")
                Task { @MainActor in self.append(.tool, name) }
            case "system":
                if let t = obj["text"] as? String, !t.isEmpty { Task { @MainActor in self.append(.system, t) } }
            case "error":
                Task { @MainActor in self.append(.error, obj["error"] as? String ?? "") }
            case "busy":
                let b = obj["busy"] as? Bool ?? false
                Task { @MainActor in self.running = b }
            default: return
            }
        }
        if id > 0 { StreamManager.shared.sendText(id, firstTurn) }
    }

    private func append(_ kind: AgentEntry.Kind, _ text: String) {
        started = true
        if kind == .text, let last = entries.last, last.kind == .text {
            entries[entries.count - 1].text += text
        } else {
            entries.append(AgentEntry(kind: kind, text: text))
        }
    }

    static func prettyTool(_ raw: String) -> String {
        guard raw.hasPrefix("mcp__") else { return raw }
        let parts = raw.dropFirst(5).components(separatedBy: "__")
        if parts.count >= 2 { return parts[0] + " · " + parts[1...].joined(separator: "__") }
        return parts.joined()
    }

    func stop() {
        if id > 0 { StreamManager.shared.close(id) }
        id = 0
    }
}

private struct AgentEntryRow: View {
    let entry: AgentEntry
    var body: some View {
        switch entry.kind {
        case .text:
            Text(Self.md(entry.text)).font(.system(size: 12)).foregroundStyle(Theme.fg)
                .lineSpacing(3).frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled)
        case .tool:
            HStack(spacing: 6) {
                Image(systemName: Self.icon(entry.text)).font(.system(size: 9.5, weight: .semibold)).foregroundStyle(Theme.accent)
                Text(entry.text).font(Theme.mono(10.5)).foregroundStyle(Theme.fgMuted)
            }
            .padding(.horizontal, 8).padding(.vertical, 3)
            .background(Theme.panel3, in: Capsule())
            .overlay(Capsule().strokeBorder(Theme.borderSoft))
        case .system:
            Text(entry.text).font(.system(size: 11)).italic().foregroundStyle(Theme.fgMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .error:
            HStack(alignment: .top, spacing: 6) {
                Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 10)).foregroundStyle(Theme.danger)
                Text(entry.text).font(Theme.mono(11)).foregroundStyle(Theme.danger)
            }.frame(maxWidth: .infinity, alignment: .leading)
        }
    }
    private static func md(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible))) ?? AttributedString(s)
    }
    private static func icon(_ tool: String) -> String {
        let t = tool.lowercased()
        if t.contains("bash") { return "terminal" }
        if t.contains("read") || t.contains("file_get") { return "doc.text" }
        if t.contains("glob") || t.contains("grep") || t.contains("search") { return "magnifyingglass" }
        if t.contains("edit") || t.contains("write") || t.contains("file_set") || t.contains("set") { return "pencil" }
        if t.contains("validate") || t.contains("doctor") { return "checkmark.seal" }
        if t.contains("config") { return "gearshape" }
        return "wrench.and.screwdriver"
    }
}

struct AgentSheet: View {
    @ObservedObject var model: AgentStreamModel
    let title: String
    let subtitle: String
    let runningLabel: String
    var onBackground: () -> Void = {}
    var onDone: () -> Void = {}
    var onStop: () -> Void = {}
    @State private var slow = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                if model.running { Spinner() }
                Text(model.running ? runningLabel : title + " done")
                    .font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.fg)
                Spacer()
            }
            Text(subtitle).font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            ScrollViewReader { proxy in
                ScrollView {
                    if !model.started {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("Starting Claude…").font(Theme.mono(11.5)).foregroundStyle(Theme.fg)
                            Text("Initializing the pom MCP tools and reading your config. On a large codebase this takes a moment before the first output.")
                                .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                            if slow {
                                Text("Still starting — Claude may be initializing, or `claude` isn't signed in yet (run `claude` once in a terminal to log in). It keeps running in the background.")
                                    .font(.system(size: 11)).foregroundStyle(Theme.warn)
                            }
                        }.padding(12).frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(model.entries) { AgentEntryRow(entry: $0) }
                            Color.clear.frame(height: 1).id("bottom")
                        }
                        .padding(12).frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .frame(minHeight: 340).background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
                .onChange(of: model.tick) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            HStack {
                if model.running {
                    Button("Stop") { onStop() }.buttonStyle(.bordered).tint(Theme.danger)
                        .help("Force-stop the agent and close.")
                }
                Spacer()
                Button(model.running ? "Run in background" : "Done") { model.running ? onBackground() : onDone() }
                    .buttonStyle(.borderedProminent).tint(Theme.accent)
            }
        }
        .padding(18).frame(width: 640, height: 520)
        .task { try? await Task.sleep(nanoseconds: 20_000_000_000); if !model.started { slow = true } }
    }
}

enum OnboardPrompts {
    static let firstTurn = "Onboard this session now: analyze every cloned repo and author a correct, complete pom.yml " +
        "(frameworks, monorepo apps, all processes, setup, shared services from every compose incl. extends, and repo " +
        "aliases). Loop config_doctor until zero errors. Then call config_normalize as the FINAL step (it strips " +
        "removed keys, migrates colon→dot, and tidies into pom.d), and confirm what you defined."
}

struct OnboardSheet: View {
    @ObservedObject var model: AgentStreamModel
    let startAt: Date
    let branch: String
    var onBackground: () -> Void = {}
    var onDone: () -> Void = {}

    @State private var findings: [DoctorViewModel.Finding] = []
    @State private var showResult = false
    @State private var endedAt: Date?
    @State private var showLog = false
    @State private var installing = false
    @State private var authorDone = false

    var body: some View {
        Group {
            if showResult { resultSheet } else { onboardingBody }
        }
        .onAppear { if !model.running && !authorDone { authorDone = true; Task { await verifyAndInstall() } } }
        .onChange(of: model.running) {
            if !model.running && !authorDone { authorDone = true; Task { await verifyAndInstall() } }
        }
    }

    private func verifyAndInstall() async {
        installing = true
        let d = await SessionStore.installDeps(branch: branch, isMain: true)
        struct Fail: Decodable { var id = ""; var title = ""; var detail = "" }
        struct R: Decodable { var ok = false; var failed: [Fail] = [] }
        let failed = PomJSON.decode(R.self, from: d)?.failed ?? []
        installing = false
        endedAt = Date()
        await loadFindings(extraSetup: failed.map { ($0.id, $0.title, $0.detail) })
    }

    private var onboardingBody: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                if model.running { Spinner() }
                Text("Onboarding · \(PomCore.shared.session)")
                    .font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.fg)
                Spacer()
                TimelineView(.periodic(from: startAt, by: 1)) { ctx in
                    HStack(spacing: 4) {
                        Image(systemName: "clock").font(.system(size: 10))
                        Text(mmss(startAt, endedAt ?? ctx.date)).font(Theme.mono(11))
                    }.foregroundStyle(Theme.fgMuted)
                }
            }
            phaseList
            Text("Now: \(nowLine)").font(.system(size: 12)).foregroundStyle(Theme.fgMuted).lineLimit(2)

            DisclosureGroup(isExpanded: $showLog) {
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(model.entries) { AgentEntryRow(entry: $0) }
                            Color.clear.frame(height: 1).id("bottom")
                        }.padding(10).frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(height: 240).background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
                    .onChange(of: model.tick) { proxy.scrollTo("bottom", anchor: .bottom) }
                }
            } label: {
                Text("Activity · \(model.entries.count) step\(model.entries.count == 1 ? "" : "s")")
                    .font(.system(size: 11.5)).foregroundStyle(Theme.accent)
            }

            Spacer(minLength: 0)
            HStack {
                Button("Stop") { onDone() }.buttonStyle(.bordered).tint(Theme.danger)
                Spacer()
                Button("Run in background") { onBackground() }.buttonStyle(.borderedProminent).tint(Theme.accent)
            }
        }
        .padding(18).frame(width: 640, height: 520)
    }

    private var phaseList: some View {
        VStack(alignment: .leading, spacing: 5) {
            phaseRow(done: true, active: false, "Scan repos", "cloned")
            phaseRow(done: authorDone, active: model.running, "Author config", model.running ? "writing pom.yml…" : "done")
            phaseRow(done: authorDone && !installing, active: installing, "Install & migrate",
                     installing ? "installing deps + migrating…" : (authorDone ? "done" : "waiting"))
        }
    }

    private func phaseRow(done: Bool, active: Bool, _ title: String, _ detail: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: done ? "checkmark.circle.fill" : (active ? "circle.dashed" : "circle"))
                .font(.system(size: 11)).foregroundStyle(done ? Theme.ok : (active ? Theme.accent : Theme.dim))
            Text(title).font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.fg)
            Text(detail).font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            Spacer()
        }
    }

    private func mmss(_ from: Date, _ to: Date) -> String {
        let s = max(0, Int(to.timeIntervalSince(from)))
        return String(format: "%d:%02d", s / 60, s % 60)
    }

    private var nowLine: String {
        guard let e = model.entries.last else { return model.started ? "working…" : "starting Claude…" }
        switch e.kind {
        case .tool: return "running \(e.text)"
        case .text: return e.text.split(separator: "\n").last.map(String.init) ?? "…"
        case .system, .error: return e.text
        }
    }

    private var resultSheet: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: "clock").font(.system(size: 10))
                Text("Onboarding done · \(mmss(startAt, endedAt ?? Date()))").font(.system(size: 12, weight: .medium))
                Spacer()
            }.foregroundStyle(Theme.fgMuted).padding(.horizontal, 16).padding(.top, 14)
            ScrollView { OnboardResultView(findings: findings, services: 0, onRetry: retry) }
            HStack {
                Spacer()
                Button("Done") { onDone() }.buttonStyle(.borderedProminent).tint(Theme.accent)
            }
            .padding(.horizontal, 16).padding(.bottom, 14)
        }
        .frame(width: 640, height: 520)
    }

    private func loadFindings(extraSetup: [(String, String, String)] = []) async {
        let d = await SessionStore.doctor()
        let report = PomJSON.decode(DoctorViewModel.Report.self, from: d)
        var all = (report?.findings ?? []).filter { $0.severity != "ok" }
        for (id, title, detail) in extraSetup {
            var f = DoctorViewModel.Finding(); f.id = id; f.title = title; f.detail = detail; f.severity = "error"
            all.append(f)
        }
        findings = all
        showResult = true
    }

    private func retry() {
        model.stop()
        showResult = false
        findings = []
        authorDone = false
        installing = false
        endedAt = nil
        model.start(branch: branch, isMain: true, role: "onboarder", firstTurn: OnboardPrompts.firstTurn)
    }
}
