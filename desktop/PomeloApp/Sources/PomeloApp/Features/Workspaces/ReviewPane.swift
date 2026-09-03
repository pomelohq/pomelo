import SwiftUI
import AppKit

// P0 review artifact: an authored narrative plus repo-qualified code anchors. The
// prose links each claim to real code via `pom://code?repo=..&path=..&start=..&end=..`
// so a click peeks that file at the range — the multi-repo take on a guided review.
struct Review: Decodable {
    var exists = false
    var id = ""
    var title = ""
    var doc = ""
    var anchors: [ReviewAnchor] = []
    var diagram: ReviewDiagram?
    var model: ReviewModel?
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        exists = try c.decodeIfPresent(Bool.self, forKey: .exists) ?? false
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? ""
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        doc = try c.decodeIfPresent(String.self, forKey: .doc) ?? ""
        anchors = try c.decodeIfPresent([ReviewAnchor].self, forKey: .anchors) ?? []
        diagram = try c.decodeIfPresent(ReviewDiagram.self, forKey: .diagram)
        model = try c.decodeIfPresent(ReviewModel.self, forKey: .model)
        if !doc.isEmpty { exists = true }
    }
    enum K: String, CodingKey { case exists, id, title, doc, anchors, diagram, model }
}

struct ReviewModel: Decodable {
    var title = ""
    var entities: [ModelEntity] = []
    var relations: [ModelRelation] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        entities = try c.decodeIfPresent([ModelEntity].self, forKey: .entities) ?? []
        relations = try c.decodeIfPresent([ModelRelation].self, forKey: .relations) ?? []
    }
    enum K: String, CodingKey { case title, entities, relations }
    var hasContent: Bool { !entities.isEmpty }
}

struct ModelEntity: Decodable, Identifiable {
    var id = ""
    var label = ""
    var repo = ""
    var path = ""
    var start = 0
    var end = 0
    var note = ""
    var changed = false
    var fields: [ModelField] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? ""
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? ""
        repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
        path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
        start = try c.decodeIfPresent(Int.self, forKey: .start) ?? 0
        end = try c.decodeIfPresent(Int.self, forKey: .end) ?? 0
        note = try c.decodeIfPresent(String.self, forKey: .note) ?? ""
        changed = try c.decodeIfPresent(Bool.self, forKey: .changed) ?? false
        fields = try c.decodeIfPresent([ModelField].self, forKey: .fields) ?? []
    }
    enum K: String, CodingKey { case id, label, repo, path, start, end, note, changed, fields }
    var hasCode: Bool { !path.isEmpty }
}

struct ModelField: Decodable, Identifiable {
    var name = ""
    var type = ""
    var key = ""
    var note = ""
    var changed = false
    var id: String { name }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        type = try c.decodeIfPresent(String.self, forKey: .type) ?? ""
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        note = try c.decodeIfPresent(String.self, forKey: .note) ?? ""
        changed = try c.decodeIfPresent(Bool.self, forKey: .changed) ?? false
    }
    enum K: String, CodingKey { case name, type, key, note, changed }
}

struct ModelRelation: Decodable {
    var from = ""
    var to = ""
    var label = ""
    var kind = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        from = try c.decodeIfPresent(String.self, forKey: .from) ?? ""
        to = try c.decodeIfPresent(String.self, forKey: .to) ?? ""
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? ""
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? ""
    }
    enum K: String, CodingKey { case from, to, label, kind }
}

// Agent-authored sequence diagram: participants are repos/services, steps are calls
// between them (each may link a code anchor). Renders the cross-repo control flow.
struct ReviewDiagram: Decodable {
    var title = ""
    var participants: [DiagramParticipant] = []
    var steps: [DiagramStep] = []
    var scopes: [DiagramScope] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        participants = try c.decodeIfPresent([DiagramParticipant].self, forKey: .participants) ?? []
        steps = try c.decodeIfPresent([DiagramStep].self, forKey: .steps) ?? []
        scopes = try c.decodeIfPresent([DiagramScope].self, forKey: .scopes) ?? []
    }
    enum K: String, CodingKey { case title, participants, steps, scopes }
    var hasContent: Bool { participants.count >= 2 && !steps.isEmpty }
}

// A boundary around a run of steps: a DB transaction, a held lock, an opt/loop
// fragment. `from`/`to` are 1-based step numbers (inclusive).
struct DiagramScope: Decodable {
    var label = ""
    var kind = "scope" // transaction | lock | opt | loop | scope
    var from = 0
    var to = 0
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? ""
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? "scope"
        from = try c.decodeIfPresent(Int.self, forKey: .from) ?? 0
        to = try c.decodeIfPresent(Int.self, forKey: .to) ?? 0
    }
    enum K: String, CodingKey { case label, kind, from, to }
}

struct DiagramParticipant: Decodable, Identifiable {
    var id = ""
    var label = ""
    var repo = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? ""
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? ""
        repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
    }
    enum K: String, CodingKey { case id, label, repo }
}

struct DiagramStep: Decodable {
    var from = ""
    var to = ""
    var label = ""
    var note = ""
    var kind = "call" // "call" (solid) or "return" (dashed)
    // Each step carries the PRECISE lines that implement this call — not a shared
    // anchor covering a whole method — so the peek matches the step exactly.
    var repo = ""
    var path = ""
    var start = 0
    var end = 0
    var anchor = "" // optional fallback: resolve a doc anchor by id
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        from = try c.decodeIfPresent(String.self, forKey: .from) ?? ""
        to = try c.decodeIfPresent(String.self, forKey: .to) ?? ""
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? ""
        note = try c.decodeIfPresent(String.self, forKey: .note) ?? ""
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? "call"
        repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
        path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
        start = try c.decodeIfPresent(Int.self, forKey: .start) ?? 0
        end = try c.decodeIfPresent(Int.self, forKey: .end) ?? 0
        anchor = try c.decodeIfPresent(String.self, forKey: .anchor) ?? ""
    }
    enum K: String, CodingKey { case from, to, label, note, kind, repo, path, start, end, anchor }
    var isReturn: Bool { kind == "return" }
    var hasCode: Bool { !path.isEmpty || !anchor.isEmpty }
}

struct ReviewAnchor: Decodable, Identifiable {
    var id = ""
    var repo = ""
    var path = ""
    var start = 0
    var end = 0
    var side = "head"
    var note = "" // one-line caption for the guided tour stop
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? ""
        repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
        path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
        start = try c.decodeIfPresent(Int.self, forKey: .start) ?? 0
        end = try c.decodeIfPresent(Int.self, forKey: .end) ?? 0
        side = try c.decodeIfPresent(String.self, forKey: .side) ?? "head"
        note = try c.decodeIfPresent(String.self, forKey: .note) ?? ""
    }
    enum K: String, CodingKey { case id, repo, path, start, end, side, note }
}


struct ReviewComment: Decodable {
    var author = ""
    var body = ""
    var createdAt = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        author = try c.decodeIfPresent(String.self, forKey: .author) ?? ""
        body = try c.decodeIfPresent(String.self, forKey: .body) ?? ""
        createdAt = try c.decodeIfPresent(String.self, forKey: .createdAt) ?? ""
    }
    enum K: String, CodingKey { case author, body, createdAt }
}

struct ReviewThread: Decodable, Identifiable {
    var id = ""
    var repo = ""
    var path = ""
    var start = 0
    var end = 0
    var side = "head"
    var resolved = false
    var comments: [ReviewComment] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? ""
        repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
        path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
        start = try c.decodeIfPresent(Int.self, forKey: .start) ?? 0
        end = try c.decodeIfPresent(Int.self, forKey: .end) ?? 0
        side = try c.decodeIfPresent(String.self, forKey: .side) ?? "head"
        resolved = try c.decodeIfPresent(Bool.self, forKey: .resolved) ?? false
        comments = try c.decodeIfPresent([ReviewComment].self, forKey: .comments) ?? []
    }
    enum K: String, CodingKey { case id, repo, path, start, end, side, resolved, comments }
}

struct CodePeekTarget: Identifiable, Equatable {
    let repo: String, path: String, start: Int, end: Int
    var id: String { repo + "/" + path + ":\(start)-\(end)" }
}

struct ReviewPane: View {
    @EnvironmentObject var theme: ThemeManager
    let workspace: Workspace
    // The pane stays mounted; reload when it becomes active so a freshly-generated
    // review shows up without a manual refresh.
    var isActive: Bool = false
    // Hands a prompt to the workspace agent: the host switches to the (persistent)
    // Claude pane and types it in for the reviewer to send.
    var onAskAgent: (String) -> Void = { _ in }

    @State private var review: Review?
    @State private var loaded = false
    @State private var lastRaw: Data?
    @State private var watchTask: Task<Void, Never>?
    @State private var peek: CodePeekTarget?
    @State private var tourIndex: Int?
    @State private var docTab: DocTab = .doc
    @State private var focusedStep: Int?
    @AppStorage("review.previewWidth") private var previewWidth = 520.0

    private enum DocTab { case doc, diagram, model }

    // The agent runs per-workspace (Cmd-I); main has none, so Ask is hidden there.
    private var canAsk: Bool { !workspace.isMain }
    private var showingFlow: Bool { docTab == .diagram && (review?.diagram?.hasContent ?? false) }

    var body: some View {
        HStack(spacing: 0) {
            docPane.frame(maxWidth: .infinity, maxHeight: .infinity)
            rightPane
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.bg)
        .task(id: workspace.id) { await load(); startWatch() }
        .onChange(of: isActive) { startWatch() }
        .onDisappear { watchTask?.cancel() }
        .perfTag("ReviewPane")
    }

    private func startWatch() {
        watchTask?.cancel()
        guard isActive else { return }
        let branch = workspace.branch, isMain = workspace.isMain
        watchTask = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                guard !Task.isCancelled else { return }
                let raw = await Task.detached(priority: .utility) { ReviewStore.get(branch: branch, isMain: isMain) }.value
                if raw != lastRaw {
                    lastRaw = raw
                    review = PomJSON.decode(Review.self, from: raw)
                    loaded = true
                }
            }
        }
    }

    // Flow view -> the step timeline on the right (with the "Open" peek OVERLAID so the
    // timeline stays mounted and keeps its scroll); Narrative -> the single code peek.
    @ViewBuilder private var rightPane: some View {
        if showingFlow, let dg = review?.diagram {
            SplitHandle(axis: .horizontal, value: $previewWidth, min: 380, max: 1000, invert: true)
            ZStack {
                FlowTourPanel(diagram: dg, anchors: review?.anchors ?? [],
                              branch: workspace.branch, isMain: workspace.isMain,
                              focused: $focusedStep, onOpenFull: openStep)
                if let t = peek {
                    CodePeekPane(target: t, branch: workspace.branch, isMain: workspace.isMain,
                                 canAsk: canAsk, onAskAgent: askAgent, onClose: { peek = nil })
                        .id(t.id)
                        .transition(.move(edge: .trailing))
                }
            }
            .frame(width: previewWidth)
        } else if let t = peek {
            SplitHandle(axis: .horizontal, value: $previewWidth, min: 340, max: 1000, invert: true)
            CodePeekPane(target: t, branch: workspace.branch, isMain: workspace.isMain,
                         canAsk: canAsk, onAskAgent: askAgent, onClose: { peek = nil })
                .frame(width: previewWidth)
                .id(t.id)
                .transition(.move(edge: .trailing))
        }
    }

    private func askAgent(_ prompt: String) { onAskAgent(prompt) }

    @ViewBuilder private var docPane: some View {
        if let r = review, r.exists {
            VStack(spacing: 0) {
                if r.diagram?.hasContent == true || r.model?.hasContent == true { docTabBar }
                if docTab == .model, let m = r.model, m.hasContent {
                    ERDiagramView(model: m, onOpenEntity: openEntity)
                } else if docTab == .diagram, let dg = r.diagram, dg.hasContent {
                    DiagramView(diagram: dg, focused: focusedStep, onSelectStep: { focusedStep = $0 })
                } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 10) {
                            if !r.title.isEmpty {
                                Text(r.title).font(.system(size: 18, weight: .semibold)).foregroundStyle(Theme.fg)
                            }
                            NarrativeText(text: r.doc, isDark: theme.mode.isDark, onLink: { _ = handleLink($0) })
                        }
                        .padding(.horizontal, 28).padding(.vertical, 24).readingColumn(760)
                    }
                    .environment(\.openURL, OpenURLAction { url in handleLink(url) })
                }
            }
        } else if !loaded {
            LoadingView(text: "loading review…")
        } else {
            VStack(spacing: 14) {
                Image(systemName: "doc.text.magnifyingglass").font(.system(size: 30)).foregroundStyle(Theme.dim)
                VStack(spacing: 4) {
                    Text("No review yet").font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.fgMuted)
                    Text("Generate one from the branch across every repo.")
                        .font(.system(size: 12)).foregroundStyle(Theme.dim)
                }
                if canAsk {
                    Button { askAgent("Use the pom-review skill to review this workspace.") } label: {
                        HStack(spacing: 6) {
                            Image(systemName: "sparkles").font(.system(size: 11))
                            Text("Generate with AI").font(.system(size: 12.5, weight: .medium))
                        }
                        .foregroundStyle(.white).padding(.horizontal, 14).padding(.vertical, 7)
                        .background(Theme.accent, in: Capsule())
                    }.buttonStyle(.plain)
                        .help("Switches to the agent and pre-fills the review command")
                        .padding(.top, 4)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // Guided tour: walk the anchors in authored order, opening each in the peek pane.
    @ViewBuilder private func tourBar(_ r: Review) -> some View {
        if !r.anchors.isEmpty {
            HStack(spacing: 10) {
                if let ti = tourIndex, ti < r.anchors.count {
                    tourStep("chevron.left", tip: "Previous stop") { go(ti - 1, r) }
                    Text("\(ti + 1) / \(r.anchors.count)").font(Theme.mono(11, .semibold)).foregroundStyle(Theme.fg)
                        .monospacedDigit()
                    tourStep("chevron.right", tip: "Next stop") { go(ti + 1, r) }
                    let note = r.anchors[ti].note
                    if !note.isEmpty {
                        Divider().frame(height: 14).overlay(Theme.borderSoft)
                        Text(note).font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                            .lineLimit(1).truncationMode(.tail).frame(maxWidth: 340, alignment: .leading)
                    }
                    tourStep("xmark", tip: "End tour") { tourIndex = nil }
                } else {
                    Image(systemName: "play.fill").font(.system(size: 10)).foregroundStyle(Theme.accent)
                    Text("Tour \(r.anchors.count) stops").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.fg)
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 9)
            .background(.ultraThinMaterial, in: Capsule())
            .overlay(Capsule().strokeBorder(Theme.borderSoft))
            .contentShape(Capsule())
            .onTapGesture { if tourIndex == nil { go(0, r) } }
            .padding(.bottom, 18)
            .shadow(color: .black.opacity(0.25), radius: 12, y: 4)
        }
    }

    private func tourStep(_ icon: String, tip: String, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            Image(systemName: icon).font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Theme.fgSoft).frame(width: 22, height: 22)
                .background(Theme.hover, in: Circle())
        }.buttonStyle(.plain).help(tip)
    }

    private func go(_ i: Int, _ r: Review) {
        guard !r.anchors.isEmpty else { return }
        let idx = (i % r.anchors.count + r.anchors.count) % r.anchors.count
        tourIndex = idx
        let a = r.anchors[idx]
        withAnimation(.easeInOut(duration: 0.16)) {
            peek = CodePeekTarget(repo: a.repo, path: a.path, start: a.start, end: a.end)
        }
    }

    private var docTabBar: some View {
        HStack(spacing: 6) {
            docTabBtn("Narrative", "doc.text", .doc)
            if review?.diagram?.hasContent == true { docTabBtn("Flow", "arrow.triangle.branch", .diagram) }
            if review?.model?.hasContent == true { docTabBtn("Model", "tablecells", .model) }
            Spacer()
        }
        .padding(.horizontal, 16).padding(.vertical, 8)
        .background(Theme.bgSoft)
        .overlay(alignment: .bottom) { Divider().overlay(Theme.borderSoft) }
    }

    private func docTabBtn(_ label: String, _ icon: String, _ tab: DocTab) -> some View {
        Button {
            withAnimation(.easeInOut(duration: 0.12)) { docTab = tab }
            if tab == .diagram, focusedStep == nil { focusedStep = 0 }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: icon).font(.system(size: 11))
                Text(label).font(.system(size: 12, weight: .medium))
            }
            .foregroundStyle(docTab == tab ? Theme.accent : Theme.fgMuted)
            .padding(.horizontal, 10).padding(.vertical, 4)
            .background(docTab == tab ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 6))
        }.buttonStyle(.plain)
    }

    private func openEntity(_ e: ModelEntity) {
        guard e.hasCode else { return }
        withAnimation(.easeInOut(duration: 0.16)) {
            peek = CodePeekTarget(repo: e.repo, path: e.path, start: e.start, end: e.end)
        }
    }

    private func openAnchor(_ id: String) {
        guard let a = review?.anchors.first(where: { $0.id == id }) else { return }
        withAnimation(.easeInOut(duration: 0.16)) {
            peek = CodePeekTarget(repo: a.repo, path: a.path, start: a.start, end: a.end)
        }
    }

    // A diagram step opens its OWN precise line range (falls back to a doc anchor).
    private func openStep(_ s: DiagramStep) {
        if !s.path.isEmpty {
            let repo = s.repo.isEmpty ? (review?.anchors.first?.repo ?? "") : s.repo
            withAnimation(.easeInOut(duration: 0.16)) {
                peek = CodePeekTarget(repo: repo, path: s.path, start: s.start, end: s.end)
            }
        } else if !s.anchor.isEmpty {
            openAnchor(s.anchor)
        }
    }

    private func handleLink(_ url: URL) -> OpenURLAction.Result {
        guard url.scheme == "pom", let comps = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            NSWorkspace.shared.open(url)
            return .handled
        }
        let q = Dictionary(comps.queryItems?.map { ($0.name, $0.value ?? "") } ?? [], uniquingKeysWith: { a, _ in a })
        switch url.host {
        case "flow":
            guard review?.diagram?.hasContent == true else { return .handled }
            withAnimation(.easeInOut(duration: 0.16)) {
                docTab = .diagram
                if let s = Int(q["step"] ?? ""), s > 0 { focusedStep = s - 1 }
                else if focusedStep == nil { focusedStep = 0 }
            }
            return .handled
        case "model":
            guard review?.model?.hasContent == true else { return .handled }
            withAnimation(.easeInOut(duration: 0.16)) { docTab = .model }
            return .handled
        case "code":
            guard let repo = q["repo"], let path = q["path"], !repo.isEmpty, !path.isEmpty else { return .handled }
            withAnimation(.easeInOut(duration: 0.16)) {
                peek = CodePeekTarget(repo: repo, path: path, start: Int(q["start"] ?? "") ?? 0, end: Int(q["end"] ?? "") ?? 0)
            }
            return .handled
        default:
            NSWorkspace.shared.open(url)
            return .handled
        }
    }

    private func load() async {
        let branch = workspace.branch, isMain = workspace.isMain
        let raw = await Task.detached(priority: .userInitiated) { ReviewStore.get(branch: branch, isMain: isMain) }.value
        lastRaw = raw
        review = PomJSON.decode(Review.self, from: raw)
        loaded = true
    }
}

// Peeks a file at a line range in a side pane next to the review (not a modal).
struct CodePeekPane: View {
    @EnvironmentObject var theme: ThemeManager
    @ObservedObject private var codeDisplay = CodeDisplayManager.shared
    let target: CodePeekTarget
    let branch: String
    let isMain: Bool
    var canAsk: Bool = false
    var onAskAgent: (String) -> Void = { _ in }
    var onClose: () -> Void

    @State private var content: String?
    @State private var threads: [ReviewThread] = []
    @State private var showThreads = false
    @State private var selLines: ClosedRange<Int>?

    private var fileThreads: [ReviewThread] {
        threads.filter { $0.repo == target.repo && $0.path == target.path }
    }

    // The note attaches to the lines the reviewer selects in the code; with no
    // selection it falls back to the anchor's own range.
    private var noteRange: ClosedRange<Int> {
        if let s = selLines { return s }
        let lo = max(1, target.start)
        return lo...max(lo, target.end)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "doc.text").font(.system(size: 11)).foregroundStyle(Theme.accent)
                Text(target.repo).font(Theme.mono(11, .semibold)).foregroundStyle(Theme.accent)
                Text(target.path).font(Theme.mono(11)).foregroundStyle(Theme.fg).lineLimit(1).truncationMode(.middle)
                if target.start > 0 { Text(":\(target.start)-\(target.end)").font(Theme.mono(10.5)).foregroundStyle(Theme.dim) }
                Spacer()
                threadsToggle
                IconButton("xmark", size: 12, tip: "Close preview") { onClose() }
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .background(Theme.bgSoft)
            Divider().overlay(Theme.borderSoft)
            if let content {
                CodeView(content: content, lang: CodeLang.detect(path: target.path),
                                 start: target.start, end: target.end, isDark: theme.mode.isDark,
                                 wrapMode: codeDisplay.wrapMode,
                                 onSelectLines: { sel in withAnimation(.easeInOut(duration: 0.12)) { selLines = sel } })
                    .id(target.id)
            } else {
                LoadingView(text: "loading file…")
            }
            if showThreads {
                Divider().overlay(Theme.borderSoft)
                ReviewThreadsView(target: target, branch: branch, isMain: isMain,
                                  noteStart: noteRange.lowerBound, noteEnd: noteRange.upperBound,
                                  threads: fileThreads, canAsk: canAsk, onAskAgent: onAskAgent,
                                  onChange: { await reloadThreads() })
                    .frame(height: 300)
            } else if selLines != nil {
                Divider().overlay(Theme.borderSoft)
                ReviewThreadsView(target: target, branch: branch, isMain: isMain,
                                  noteStart: noteRange.lowerBound, noteEnd: noteRange.upperBound,
                                  threads: fileThreads, canAsk: canAsk, onAskAgent: onAskAgent,
                                  compact: true, onChange: { await reloadThreads() })
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Theme.bg)
        .task(id: target.id) { selLines = nil; await load() }
    }

    private var threadsToggle: some View {
        Button { withAnimation(.easeInOut(duration: 0.14)) { showThreads.toggle() } } label: {
            HStack(spacing: 4) {
                Image(systemName: "bubble.left.and.bubble.right").font(.system(size: 11))
                if !fileThreads.isEmpty {
                    Text("\(fileThreads.count)").font(.system(size: 10, weight: .semibold)).monospacedDigit()
                }
            }
            .foregroundStyle(showThreads ? Theme.accent : Theme.fgMuted)
            .padding(.horizontal, 6).padding(.vertical, 3)
            .background(showThreads ? Theme.accentSoft : .clear, in: RoundedRectangle(cornerRadius: 6))
        }.buttonStyle(.plain).help("Review notes on this file")
    }

    private func load() async {
        let t = target, branch = branch, isMain = isMain
        let text = await Task.detached(priority: .userInitiated) { () -> String? in
            struct R: Decodable { var content: String?; var error: String? }
            guard let r = PomJSON.decode(R.self, from: ReviewStore.peek(branch: branch, repo: t.repo, path: t.path, isMain: isMain)),
                  (r.error ?? "").isEmpty, let c = r.content else { return nil }
            return c
        }.value
        content = text ?? "(file not found)"
        await reloadThreads()
    }

    private func reloadThreads() async {
        let branch = branch, isMain = isMain
        let list = await Task.detached(priority: .userInitiated) { () -> [ReviewThread] in
            struct R: Decodable { var threads: [ReviewThread] = [] }
            return PomJSON.decode(R.self, from: ReviewStore.threads(branch: branch, isMain: isMain))?.threads ?? []
        }.value
        threads = list
    }
}

// Thread list + composer for the file shown in the peek. Comments anchor to the
// current line range; optionally posted to GitHub as a PR review comment.
struct ReviewThreadsView: View {
    @EnvironmentObject var theme: ThemeManager
    let target: CodePeekTarget
    let branch: String
    let isMain: Bool
    let noteStart: Int
    let noteEnd: Int
    let threads: [ReviewThread]
    var canAsk: Bool = false
    var onAskAgent: (String) -> Void = { _ in }
    var compact = false
    var onChange: () async -> Void

    @State private var draft = ""
    @State private var sending = false
    @State private var replyTo: String?
    @State private var replyDraft = ""
    @State private var error = ""
    @State private var collapsed: Set<String> = []
    @State private var showAllComments: Set<String> = []

    private static let commentPeek = 4

    var body: some View {
        if compact {
            composer
        } else {
            fullList
        }
    }

    private var fullList: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    if threads.isEmpty {
                        Text("No notes yet. Select lines in the code, then note what to check.")
                            .font(.system(size: 11.5)).foregroundStyle(Theme.dim).padding(.top, 4)
                    }
                    ForEach(threads) { thread in threadCard(thread) }
                }
                .padding(12)
            }
            Divider().overlay(Theme.borderSoft)
            composer
        }
        .background(Theme.bg)
        .task(id: threadSeed) { collapsed = Set(threads.filter(\.resolved).map(\.id)) }
    }

    // Resolved threads start collapsed; re-seed when the set or a resolved-state flips.
    private var threadSeed: String { threads.map { "\($0.id):\($0.resolved)" }.joined(separator: ",") }

    private func threadCard(_ t: ReviewThread) -> some View {
        let isCollapsed = collapsed.contains(t.id)
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Button {
                    withAnimation(.easeInOut(duration: 0.12)) {
                        if isCollapsed { collapsed.remove(t.id) } else { collapsed.insert(t.id) }
                    }
                } label: {
                    Image(systemName: isCollapsed ? "chevron.right" : "chevron.down")
                        .font(.system(size: 9, weight: .semibold)).foregroundStyle(Theme.dim).frame(width: 12)
                }.buttonStyle(.plain).help(isCollapsed ? "Expand thread" : "Collapse thread")
                Image(systemName: "text.bubble").font(.system(size: 9)).foregroundStyle(Theme.accent)
                Text("L\(t.start)\(t.end > t.start ? "-\(t.end)" : "")").font(Theme.mono(10)).foregroundStyle(Theme.dim)
                if t.comments.count > 1 {
                    Text("\(t.comments.count)").font(.system(size: 9.5, weight: .semibold)).monospacedDigit()
                        .foregroundStyle(Theme.dim).padding(.horizontal, 5).padding(.vertical, 1)
                        .background(Theme.bg, in: Capsule())
                }
                if t.resolved { StatusPill(text: "resolved", color: Theme.ok) }
                Spacer()
                Button(t.resolved ? "Reopen" : "Resolve") { Task { await resolve(t, !t.resolved) } }
                    .buttonStyle(.plain).font(.system(size: 10)).foregroundStyle(Theme.fgMuted)
            }
            if isCollapsed {
                if let first = t.comments.first {
                    Text(first.body).font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
                        .lineLimit(1).truncationMode(.tail)
                }
            } else {
                commentList(t)
                replyControls(t)
            }
        }
        .padding(10)
        .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
        .opacity(t.resolved ? 0.65 : 1)
    }

    @ViewBuilder private func commentList(_ t: ReviewThread) -> some View {
        let comments = t.comments
        if comments.count <= Self.commentPeek || showAllComments.contains(t.id) {
            ForEach(Array(comments.enumerated()), id: \.offset) { _, c in commentRow(c) }
        } else {
            commentRow(comments[0])
            Button { withAnimation(.easeInOut(duration: 0.12)) { _ = showAllComments.insert(t.id) } } label: {
                Text("Show \(comments.count - Self.commentPeek) earlier replies")
                    .font(.system(size: 10.5, weight: .medium)).foregroundStyle(Theme.accent)
            }.buttonStyle(.plain)
            ForEach(Array(comments.suffix(Self.commentPeek - 1).enumerated()), id: \.offset) { _, c in commentRow(c) }
        }
    }

    private func commentRow(_ c: ReviewComment) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(c.author).font(.system(size: 10.5, weight: .semibold)).foregroundStyle(Theme.fgMuted)
            Text(c.body).font(.system(size: 12)).foregroundStyle(Theme.fgSoft)
                .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
        }
    }

    @ViewBuilder private func replyControls(_ t: ReviewThread) -> some View {
        if replyTo == t.id {
            HStack(spacing: 6) {
                TextField("Reply...", text: $replyDraft, axis: .vertical).textFieldStyle(.plain)
                    .font(.system(size: 12)).padding(6)
                    .background(Theme.bg, in: RoundedRectangle(cornerRadius: 6))
                Button("Send") { Task { await reply(t) } }.buttonStyle(.plain)
                    .font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.accent)
                    .disabled(replyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        } else {
            Button("Reply") { replyTo = t.id; replyDraft = "" }.buttonStyle(.plain)
                .font(.system(size: 10.5)).foregroundStyle(Theme.accent)
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 6) {
            if !error.isEmpty {
                Text(error).font(.system(size: 10.5)).foregroundStyle(Theme.danger).lineLimit(2)
            }
            TextField("Note or question on lines \(noteStart)-\(noteEnd)…", text: $draft, axis: .vertical)
                .textFieldStyle(.plain).font(.system(size: 12)).lineLimit(1...4).padding(8)
                .background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
            HStack(spacing: 8) {
                Spacer()
                if sending { Spinner(size: 12) }
                Button { Task { await send() } } label: {
                    Text("Add note").font(.system(size: 12, weight: .medium))
                        .foregroundStyle(Theme.fgSoft).padding(.horizontal, 12).padding(.vertical, 5)
                        .background(Theme.hover, in: RoundedRectangle(cornerRadius: 7))
                }.buttonStyle(.plain)
                    .disabled(sending || isEmptyDraft)
                if canAsk {
                    Button { ask() } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "sparkles").font(.system(size: 10))
                            Text("Ask agent").font(.system(size: 12, weight: .medium))
                        }
                        .foregroundStyle(.white).padding(.horizontal, 12).padding(.vertical, 5)
                        .background(Theme.accent, in: RoundedRectangle(cornerRadius: 7))
                    }.buttonStyle(.plain).disabled(isEmptyDraft)
                }
            }
        }
        .padding(12)
        .background(Theme.bgSoft)
    }

    private var isEmptyDraft: Bool { draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    // Hand the selected lines + question to the workspace agent panel; the agent has
    // repo access, so a compact reference is enough.
    private func ask() {
        let q = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return }
        onAskAgent("[\(target.repo)/\(target.path):\(noteStart)-\(noteEnd)] \(q)")
        draft = ""
    }

    private func send() async {
        let body = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return }
        sending = true; error = ""
        let t = target, branch = branch, isMain = isMain, s = noteStart, e = noteEnd
        let res = await Task.detached(priority: .userInitiated) {
            ReviewStore.addNote(branch: branch, isMain: isMain, repo: t.repo, path: t.path, start: s, end: e, body: body)
        }.value
        sending = false
        if (res["ok"] as? Bool) != true { error = (res["error"] as? String) ?? "failed" }
        else { draft = "" }
        await onChange()
    }

    private func reply(_ t: ReviewThread) async {
        let body = replyDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return }
        let branch = branch, isMain = isMain, id = t.id
        await Task.detached(priority: .userInitiated) {
            ReviewStore.reply(branch: branch, isMain: isMain, id: id, body: body)
        }.value
        replyTo = nil; replyDraft = ""
        await onChange()
    }

    private func resolve(_ t: ReviewThread, _ resolved: Bool) async {
        let branch = branch, isMain = isMain, id = t.id
        await Task.detached(priority: .userInitiated) {
            ReviewStore.resolve(branch: branch, isMain: isMain, id: id, resolved: resolved)
        }.value
        await onChange()
    }
}


// Sequence diagram of the change: participant columns (repos/services) with lifelines,
// one numbered arrow per call. Clicking a step opens its code anchor in the peek.
struct DiagramView: View {
    @EnvironmentObject var theme: ThemeManager
    let diagram: ReviewDiagram
    var focused: Int?
    var onSelectStep: (Int) -> Void

    // Fixed geometry (not fit-to-width) so participants never squish; the canvas
    // scrolls both ways like a real sequence diagram.
    private let headH: CGFloat = 74
    private let rowH: CGFloat = 66
    private let gap: CGFloat = 210
    private let sideMargin: CGFloat = 120
    private let boxW: CGFloat = 180
    private let circleR: CGFloat = 12
    private enum Dir { case left, right }

    private func cx(_ i: Int) -> CGFloat { sideMargin + gap * CGFloat(i) }

    var body: some View {
        let n = max(1, diagram.participants.count)
        let idx = Dictionary(uniqueKeysWithValues: diagram.participants.enumerated().map { ($1.id, $0) })
        let contentW = sideMargin * 2 + gap * CGFloat(max(0, n - 1))
        // Precompute each step's center Y with extra BUFFER where scopes start/end so a
        // fragment box + its tab never collide with step content or a neighbouring box.
        let tabBuffer: CGFloat = 42, endBuffer: CGFloat = 20
        var ys: [CGFloat] = []
        var cursor = headH
        for k in 0..<diagram.steps.count {
            let starts = diagram.scopes.filter { max(1, $0.from) - 1 == k }.count
            cursor += CGFloat(starts) * tabBuffer
            ys.append(cursor + rowH * 0.5)
            cursor += rowH
            let ends = diagram.scopes.filter { min(max($0.from, $0.to), diagram.steps.count) - 1 == k }.count
            cursor += CGFloat(ends) * endBuffer
        }
        let contentH = cursor + 24
        return GeometryReader { geo in
          ScrollView([.horizontal, .vertical]) {
            ZStack(alignment: .topLeading) {
                Canvas { ctx, _ in
                    for i in 0..<n {
                        var p = Path()
                        p.move(to: CGPoint(x: cx(i), y: headH - 8))
                        p.addLine(to: CGPoint(x: cx(i), y: contentH - 16))
                        ctx.stroke(p, with: .color(Theme.borderSoft), style: StrokeStyle(lineWidth: 1, dash: [3, 4]))
                    }
                    // Boundary fragments (transaction / held lock / opt): a labeled box
                    // around the run of steps inside that scope, so the span is obvious.
                    func rowY(_ k: Int) -> CGFloat { k < ys.count ? ys[k] : headH + rowH * CGFloat(k) }
                    for sc in diagram.scopes {
                        let lo = max(1, sc.from) - 1, hi = min(max(sc.from, sc.to), diagram.steps.count) - 1
                        guard lo >= 0, hi >= lo, hi < diagram.steps.count else { continue }
                        // horizontal extent = participants touched by the scoped steps
                        var cols = Set<Int>()
                        for k in lo...hi {
                            if let fi = idx[diagram.steps[k].from] { cols.insert(fi) }
                            if let ti = idx[diagram.steps[k].to] { cols.insert(ti) }
                        }
                        guard let cMin = cols.min(), let cMax = cols.max() else { continue }
                        // Nesting depth: how many other scopes strictly contain this one. Inset
                        // by depth so a nested box sits INSIDE its parent with a visible gap
                        // instead of sharing an edge.
                        let depth = diagram.scopes.filter { o in
                            let oLo = max(1, o.from), oHi = max(o.from, o.to)
                            let sLo = max(1, sc.from), sHi = max(sc.from, sc.to)
                            return oLo <= sLo && oHi >= sHi && (oLo < sLo || oHi > sHi)
                        }.count
                        let ins = CGFloat(depth)
                        let xL = cx(cMin) - 44 + ins * 12, xR = cx(cMax) + 44 - ins * 12
                        // Base top clears the step label; nested boxes inset only slightly so
                        // the top border never drops onto the label.
                        let yT = rowY(lo) - 36 + ins * 6, yB = rowY(hi) + 28 - ins * 6
                        let active = focused.map { $0 >= lo && $0 <= hi } ?? false
                        let rect = CGRect(x: xL, y: yT, width: xR - xL, height: yB - yT)
                        let rr = Path(roundedRect: rect, cornerRadius: 8)
                        if active { ctx.fill(rr, with: .color(Theme.wsAccent.opacity(0.08))) }
                        ctx.stroke(rr, with: .color(Theme.wsAccent.opacity(active ? 0.95 : 0.5)),
                                   style: StrokeStyle(lineWidth: active ? 1.6 : 1, dash: [4, 3]))
                        // UML combined-fragment tab: a pentagon (cut bottom-right) sitting ABOVE
                        // the box border so it never overlaps the first step's label.
                        let tag = sc.kind + (sc.label.isEmpty ? "" : ": " + sc.label)
                        let tw = CGFloat(tag.count) * 6.2 + 18, th: CGFloat = 18, cut: CGFloat = 7
                        let top = yT - th
                        var pent = Path()
                        pent.move(to: CGPoint(x: xL, y: top))
                        pent.addLine(to: CGPoint(x: xL + tw, y: top))
                        pent.addLine(to: CGPoint(x: xL + tw, y: yT - cut))
                        pent.addLine(to: CGPoint(x: xL + tw - cut, y: yT))
                        pent.addLine(to: CGPoint(x: xL, y: yT))
                        pent.closeSubpath()
                        ctx.fill(pent, with: .color(Theme.wsAccent.opacity(0.92)))
                        ctx.draw(Text(tag).font(Theme.mono(10, .semibold)).foregroundColor(.white),
                                 at: CGPoint(x: xL + 8, y: top + th / 2), anchor: .leading)
                    }
                    // Execution/activation bars: thin box on a lifeline for the span it is active.
                    for i in 0..<n {
                        let pid = diagram.participants[i].id
                        let rows = diagram.steps.enumerated().filter { $0.element.from == pid || $0.element.to == pid }.map { $0.offset }
                        guard let a = rows.min(), let b = rows.max(), b > a else { continue }
                        let barW: CGFloat = 8
                        let bar = CGRect(x: cx(i) - barW / 2, y: rowY(a) - 8, width: barW, height: rowY(b) - rowY(a) + 16)
                        ctx.fill(Path(roundedRect: bar, cornerRadius: 2), with: .color(Theme.accent.opacity(0.16)))
                        ctx.stroke(Path(roundedRect: bar, cornerRadius: 2), with: .color(Theme.accent.opacity(0.3)), lineWidth: 0.75)
                    }
                    for (k, s) in diagram.steps.enumerated() {
                        guard let fi = idx[s.from], let ti = idx[s.to] else { continue }
                        let y = rowY(k)
                        let x1 = cx(fi), x2 = cx(ti)
                        // Return messages are dashed and muted (UML convention); calls solid.
                        let isF = focused == k
                        let col: Color = s.isReturn ? Theme.fgMuted : Theme.accent
                        let style = StrokeStyle(lineWidth: isF ? 2.0 : (s.isReturn ? 1.3 : 1.5), lineCap: .round, dash: s.isReturn ? [5, 3] : [])
                        if fi == ti {
                            // UML self-message: leaves the node edge, loops right, returns INTO
                            // the node edge (both ends attached to the lifeline node).
                            let ex = x1 + circleR
                            let w: CGFloat = 26, h: CGFloat = 16, r: CGFloat = 5
                            let top = y - h / 2, bot = y + h / 2
                            var p = Path()
                            p.move(to: CGPoint(x: ex, y: top))
                            p.addLine(to: CGPoint(x: ex + w - r, y: top))
                            p.addQuadCurve(to: CGPoint(x: ex + w, y: top + r), control: CGPoint(x: ex + w, y: top))
                            p.addLine(to: CGPoint(x: ex + w, y: bot - r))
                            p.addQuadCurve(to: CGPoint(x: ex + w - r, y: bot), control: CGPoint(x: ex + w, y: bot))
                            p.addLine(to: CGPoint(x: ex, y: bot))
                            ctx.stroke(p, with: .color(col), style: style)
                            arrowHead(ctx, at: CGPoint(x: ex, y: bot), dir: .left, color: col)
                            ctx.draw(label(s.label), at: CGPoint(x: ex + w + 12, y: y), anchor: .leading)
                        } else {
                            let dir: Dir = x2 > x1 ? .right : .left
                            let sign: CGFloat = dir == .right ? 1 : -1
                            let startX = x1 + sign * (circleR + 5)   // gap so the node reads separate from the line
                            let endX = x2 - sign * 3                 // stop just shy of the target lifeline
                            var p = Path()
                            p.move(to: CGPoint(x: startX, y: y))
                            p.addLine(to: CGPoint(x: endX, y: y))
                            ctx.stroke(p, with: .color(col), style: style)
                            arrowHead(ctx, at: CGPoint(x: endX, y: y), dir: dir, color: col)
                            // Left-anchored above the arrow start so a long label never crosses the node.
                            ctx.draw(label(s.label), at: CGPoint(x: startX, y: y - 13), anchor: .leading)
                        }
                        let c = CGPoint(x: x1, y: y)
                        let ring = Path(ellipseIn: CGRect(x: c.x - circleR, y: c.y - circleR, width: circleR * 2, height: circleR * 2))
                        ctx.fill(ring, with: .color(isF ? Theme.accent : Theme.bg))
                        ctx.stroke(ring, with: .color(col), lineWidth: 1.5)
                        ctx.draw(Text("\(k + 1)").font(.system(size: 11, weight: .bold)).foregroundColor(isF ? .white : col), at: c)
                    }
                }
                ForEach(Array(diagram.participants.enumerated()), id: \.offset) { i, p in
                    participantBox(p).frame(width: boxW).position(x: cx(i), y: headH / 2)
                }
                ForEach(Array(diagram.steps.enumerated()), id: \.offset) { k, s in
                    do {
                        Button { onSelectStep(k) } label: {
                            RoundedRectangle(cornerRadius: 6)
                                .fill(focused == k ? Theme.accent.opacity(0.08) : Color.clear)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .onHover { if $0 { onSelectStep(k) } }
                        .frame(width: contentW, height: rowH)
                        .position(x: contentW / 2, y: k < ys.count ? ys[k] : headH + rowH * CGFloat(k))
                        .help(s.note.isEmpty ? "Step \(k + 1): \(s.label)" : s.note)
                    }
                }
            }
            .frame(width: contentW, height: contentH)
            .padding(.trailing, 40)
            // Fill at least the viewport, top-left aligned, so a small diagram pins to
            // the top-left instead of the bidirectional ScrollView centering it.
            .frame(minWidth: geo.size.width, minHeight: geo.size.height, alignment: .topLeading)
          }
          .background(Theme.bg)
        }
    }

    private func label(_ s: String) -> Text {
        Text(s).font(.system(size: 11.5, weight: .medium)).foregroundColor(Theme.fg)
    }

    private func participantBox(_ p: DiagramParticipant) -> some View {
        VStack(spacing: 1) {
            Text(p.label).font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.fg)
                .lineLimit(1).minimumScaleFactor(0.8)
            if !p.repo.isEmpty {
                Text(p.repo).font(Theme.mono(9)).foregroundStyle(Theme.dim).lineLimit(1)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .frame(width: boxW)
        .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.borderSoft))
    }

    private func arrowHead(_ ctx: GraphicsContext, at pt: CGPoint, dir: Dir, color: Color) {
        let s: CGFloat = 5.5
        let sign: CGFloat = dir == .right ? -1 : 1
        var p = Path()
        p.move(to: pt)
        p.addLine(to: CGPoint(x: pt.x + sign * s, y: pt.y - s * 0.7))
        p.addLine(to: CGPoint(x: pt.x + sign * s, y: pt.y + s * 0.7))
        p.closeSubpath()
        ctx.fill(p, with: .color(color))
    }
}

@MainActor final class FlowFileCache {
    static let shared = FlowFileCache()
    var files: [String: [(text: String, spans: [SynSpan])]] = [:]
}

// The Flow "preview": a vertical timeline of EVERY step (not one peek). Each stop
// shows its precise code slice; selecting a step in the diagram scrolls here.
struct FlowTourPanel: View {
    @EnvironmentObject var theme: ThemeManager
    let diagram: ReviewDiagram
    let anchors: [ReviewAnchor]
    let branch: String
    let isMain: Bool
    @Binding var focused: Int?
    var onOpenFull: (DiagramStep) -> Void

    @State private var files: [String: [(text: String, spans: [SynSpan])]] = [:]

    // A step's effective code range: its own precise lines, or a doc anchor fallback
    // (older artifacts wired steps to shared anchors instead of per-step ranges).
    private func coords(_ s: DiagramStep) -> (repo: String, path: String, start: Int, end: Int) {
        if !s.path.isEmpty { return (s.repo, s.path, s.start, s.end) }
        if let a = anchors.first(where: { $0.id == s.anchor }) { return (a.repo, a.path, a.start, a.end) }
        return ("", "", 0, 0)
    }

    private func key(_ s: DiagramStep) -> String { let c = coords(s); return c.repo + "/" + c.path }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(diagram.steps.enumerated()), id: \.offset) { k, s in
                        stepCard(k, s).id("tour-\(k)")
                    }
                }
                .padding(.top, 12).padding(.bottom, 64) // clears the floating pager
            }
            .contentMargins(.top, 16, for: .scrollContent)
            .onChange(of: focused) { _ in
                guard let f = focused else { return }
                withAnimation(.easeInOut(duration: 0.2)) { proxy.scrollTo("tour-\(f)", anchor: .top) }
            }
        }
        .background(Theme.bg)
        .overlay(alignment: .bottom) { pager }
        .task(id: diagram.steps.count) { await loadFiles() }
    }

    private var pager: some View {
        let n = diagram.steps.count
        let cur = focused ?? 0
        return HStack(spacing: 12) {
            pagerBtn("chevron.up") { step(-1) }.disabled(cur <= 0)
            Text("\(cur + 1) / \(n)").font(Theme.mono(11, .semibold)).foregroundStyle(Theme.fg).monospacedDigit()
            pagerBtn("chevron.down") { step(1) }.disabled(cur >= n - 1)
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(.ultraThinMaterial, in: Capsule())
        .overlay(Capsule().strokeBorder(Theme.borderSoft))
        .shadow(color: .black.opacity(0.25), radius: 10, y: 3)
        .padding(.bottom, 14)
    }

    private func pagerBtn(_ icon: String, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            Image(systemName: icon).font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Theme.fgSoft).frame(width: 22, height: 22)
                .background(Theme.hover, in: Circle())
        }.buttonStyle(.plain)
    }

    private func step(_ d: Int) {
        let n = diagram.steps.count
        guard n > 0 else { return }
        focused = min(max((focused ?? 0) + d, 0), n - 1)
    }

    private func stepCard(_ k: Int, _ s: DiagramStep) -> some View {
        let isF = focused == k
        return HStack(alignment: .top, spacing: 12) {
            ZStack(alignment: .top) {
                Rectangle().fill(Theme.borderSoft).frame(width: 2).padding(.top, 11)
                Circle().fill(isF ? Theme.accent : Theme.bgSoft)
                    .overlay(Circle().strokeBorder(Theme.accent, lineWidth: isF ? 0 : 1.5))
                    .frame(width: 22, height: 22)
                    .overlay(Text("\(k + 1)").font(.system(size: 11, weight: .bold)).foregroundStyle(isF ? .white : Theme.accent))
            }
            .frame(width: 22)
            VStack(alignment: .leading, spacing: 5) {
                Text("STEP \(k + 1) OF \(diagram.steps.count)")
                    .font(Theme.mono(9.5, .semibold)).foregroundStyle(Theme.dim).kerning(0.6)
                Text(s.label).font(Theme.mono(13, .semibold)).foregroundStyle(Theme.fg)
                    .fixedSize(horizontal: false, vertical: true)
                Text("\(participantLabel(s.from)) -> \(participantLabel(s.to))")
                    .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                if !s.note.isEmpty {
                    Text(s.note).font(.system(size: 11)).foregroundStyle(Theme.dim)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if s.hasCode { codeBox(s) }
            }
            .padding(.bottom, 22)
        }
        .padding(.horizontal, 14)
        .background(isF ? Theme.accent.opacity(0.06) : .clear)
        .contentShape(Rectangle())
        .onTapGesture { focused = k }
    }

    private func codeBox(_ s: DiagramStep) -> some View {
        let c = coords(s)
        return VStack(spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: "chevron.right").font(.system(size: 8)).foregroundStyle(Theme.dim)
                Text(c.repo).font(Theme.mono(9.5, .semibold)).foregroundStyle(Theme.accent)
                Text(c.path).font(Theme.mono(9.5)).foregroundStyle(Theme.fgMuted)
                    .lineLimit(1).truncationMode(.middle)
                if c.start > 0 { Text(":\(c.start)-\(c.end)").font(Theme.mono(9.5)).foregroundStyle(Theme.dim) }
                Spacer()
                Button { onOpenFull(s) } label: {
                    Text("Open").font(.system(size: 10, weight: .medium)).foregroundStyle(Theme.accent)
                }.buttonStyle(.plain).help("Open full file in peek")
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .background(Theme.bgSoft)
            Divider().overlay(Theme.borderSoft)
            codeLines(s)
        }
        .background(Theme.bg)
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.borderSoft))
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .padding(.top, 4)
    }

    @ViewBuilder private func codeLines(_ s: DiagramStep) -> some View {
        let c = coords(s)
        if let lines = files[key(s)] {
            if lines.isEmpty {
                Text("(file not found — check the repo/path)").font(.system(size: 10.5))
                    .foregroundStyle(Theme.dim).padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                let lo = max(1, c.start), hi = min(max(lo, c.end), lines.count)
                ScrollView(.horizontal, showsIndicators: false) {
                    VStack(alignment: .leading, spacing: 1) {
                        ForEach(lo...max(lo, hi), id: \.self) { n in
                            if n - 1 < lines.count {
                                HStack(spacing: 0) {
                                    Text("\(n)").font(Theme.mono(9.5)).foregroundStyle(Theme.dim)
                                        .frame(width: 34, alignment: .trailing).padding(.trailing, 8)
                                    highlighted(lines[n - 1].text, spans: lines[n - 1].spans)
                                        .textSelection(.enabled).lineLimit(1)
                                        .fixedSize(horizontal: true, vertical: false)
                                }
                            }
                        }
                    }
                    .padding(.vertical, 6).padding(.leading, 4).padding(.trailing, 12)
                }
            }
        } else {
            HStack { Spinner(size: 11); Text("loading…").font(.system(size: 10)).foregroundStyle(Theme.dim) }
                .padding(8)
        }
    }

    private func participantLabel(_ id: String) -> String {
        diagram.participants.first { $0.id == id }?.label ?? id
    }

    private func highlighted(_ text: String, spans: [SynSpan]) -> Text {
        SyntaxStyle.text(text, spans: spans, size: 10.5)
    }

    private func loadFiles() async {
        var uniq: [(repo: String, path: String)] = []
        var seen = Set<String>()
        for s in diagram.steps {
            let c = coords(s)
            guard !c.path.isEmpty else { continue }
            let k = c.repo + "/" + c.path
            if !seen.contains(k) { seen.insert(k); uniq.append((c.repo, c.path)) }
        }
        let branch = branch, isMain = isMain
        for u in uniq {
            let k = u.repo + "/" + u.path
            if files[k] != nil { continue }
            let ck = branch + "|" + k
            if let cached = FlowFileCache.shared.files[ck] { files[k] = cached; continue }
            let rows = await Task.detached(priority: .userInitiated) { () -> [(String, [SynSpan])]? in
                struct R: Decodable { var content: String?; var error: String? }
                guard let r = PomJSON.decode(R.self, from: ReviewStore.peek(branch: branch, repo: u.repo, path: u.path, isMain: isMain)),
                      (r.error ?? "").isEmpty, let c = r.content else { return nil }
                let lang = CodeLang.detect(path: u.path)
                let ls = c.components(separatedBy: "\n")
                let sp = Syntax.tokenize(c, lang: lang)
                return Array(zip(ls, sp)).map { ($0.0, $0.1) }
            }.value
            if let rows { files[k] = rows; FlowFileCache.shared.files[ck] = rows }
        }
    }
}
