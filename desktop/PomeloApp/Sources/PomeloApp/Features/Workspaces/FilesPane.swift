import SwiftUI
import AppKit

private struct FileContentResponse: Decodable {
    var mimeType: String
    var text: String?
    var base64: String?
    var binary: Bool
    var error: String?

    enum CodingKeys: String, CodingKey {
        case text, base64, binary, error
        case mimeType = "mime_type"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        mimeType = try c.decodeIfPresent(String.self, forKey: .mimeType) ?? ""
        text = try c.decodeIfPresent(String.self, forKey: .text)
        base64 = try c.decodeIfPresent(String.self, forKey: .base64)
        binary = try c.decodeIfPresent(Bool.self, forKey: .binary) ?? false
        error = try c.decodeIfPresent(String.self, forKey: .error)
    }
}

private enum FilePreview {
    case loading
    case text(String)
    case image(NSImage)
    case unsupported(String)
    case failed(String)
}

struct FilesPane: View {
    @EnvironmentObject var theme: ThemeManager
    let workspace: Workspace
    var onAskAgent: (String) -> Void = { _ in }
    @Binding var openRequest: WorkspaceFileEntry?

    @State private var entries: [WorkspaceFileEntry]?
    @State private var roots: [WFileTreeNode] = []
    @State private var selected: WorkspaceFileEntry?
    @State private var preview: FilePreview = .loading
    @State private var treeVisible = true
    @State private var selLines: ClosedRange<Int>?
    @State private var question = ""
    @State private var markdownRaw = false
    @State private var expanded: Set<String> = []
    @State private var streamID: Int32 = 0
    @State private var treeVersion = 0
    @State private var editText = ""
    @State private var savedText = ""

    private var selectedIsMarkdown: Bool {
        guard let path = selected?.path else { return false }
        let ext = (path as NSString).pathExtension.lowercased()
        return ext == "md" || ext == "markdown"
    }

    private var isTextPreview: Bool { if case .text = preview { return true }; return false }
    private var dirty: Bool { isTextPreview && editText != savedText }

    var body: some View {
        GeometryReader { geo in
            let overlayTree = geo.size.width < PaneMetrics.diffTreeOverlayWidth
            HStack(spacing: 0) {
                if treeVisible && !overlayTree {
                    tree
                        .frame(width: min(260, geo.size.width * 0.4))
                    Divider().overlay(Theme.borderSoft)
                }
                VStack(spacing: 0) {
                    topBar(compact: overlayTree)
                    Divider().overlay(Theme.borderSoft)
                    content
                }
            }
            .overlay(alignment: .leading) {
                if treeVisible && overlayTree {
                    floatingTree(width: min(260, geo.size.width * 0.8))
                }
            }
        }
        .background(Theme.bg)
        .background {
            Button("") { save() }.keyboardShortcut("s", modifiers: .command)
                .opacity(0).allowsHitTesting(false)
        }
        .onAppear { startWatch() }
        .onDisappear { stopWatch() }
        .task(id: selected?.id) { selLines = nil; question = ""; await loadPreview() }
        .onChange(of: openRequest) { req in
            guard let e = req else { return }
            revealInTree(e)
            selected = e
            openRequest = nil
        }
    }

    // Expand the tree down to a file so a Cmd+P jump reveals it.
    private func revealInTree(_ e: WorkspaceFileEntry) {
        expanded.insert("")
        var id = ""
        if !e.repo.isEmpty { expanded.insert(e.repo); id = e.repo }
        for part in e.path.split(separator: "/").dropLast() {
            id = id.isEmpty ? String(part) : id + "/" + part
            expanded.insert(id)
        }
    }

    private func save() {
        guard dirty, let sel = selected else { return }
        let rel = sel.repo.isEmpty ? sel.path : sel.repo + "/" + sel.path
        let abs = (workspace.path as NSString).appendingPathComponent(rel)
        do {
            try editText.write(toFile: abs, atomically: true, encoding: .utf8)
            savedText = editText
        } catch {
            let alert = NSAlert(); alert.messageText = "Couldn't save"
            alert.informativeText = error.localizedDescription
            alert.alertStyle = .warning; alert.addButton(withTitle: "OK"); alert.runModal()
        }
    }

    private var tree: some View {
        Group {
            if let entries {
                if entries.isEmpty {
                    EmptyStateView(icon: "folder", title: "No files")
                } else {
                    WorkspaceFileTreeList(roots: roots, workspacePath: workspace.path, treeVersion: treeVersion,
                                          selected: $selected, expanded: $expanded)
                }
            } else {
                LoadingView(text: "loading files…")
            }
        }
    }

    private func floatingTree(width: CGFloat) -> some View {
        HStack(spacing: 0) {
            tree
                .frame(width: width)
                .background(Theme.bgSoft)
                .overlay(alignment: .trailing) { Rectangle().fill(Theme.border).frame(width: 1) }
                .shadow(color: .black.opacity(0.28), radius: 12, x: 4)
            Color.black.opacity(0.001)
                .contentShape(Rectangle())
                .onTapGesture { withAnimation(.easeInOut(duration: 0.14)) { treeVisible = false } }
        }
        .transition(.move(edge: .leading))
    }

    private func topBar(compact: Bool) -> some View {
        HStack(spacing: 8) {
            Button { withAnimation(.easeInOut(duration: 0.14)) { treeVisible.toggle() } } label: {
                Image(systemName: "sidebar.left").font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
            }.buttonStyle(.plain).help(treeVisible ? "Hide file list" : "Show file list")
            if let selected {
                if !selected.repo.isEmpty {
                    Text(selected.repo).font(Theme.mono(11, .semibold)).foregroundStyle(Theme.accent)
                }
                Text(selected.path).font(Theme.mono(11)).foregroundStyle(Theme.fg)
                    .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
            } else {
                Text("Select a file").font(.system(size: 12)).foregroundStyle(Theme.dim)
            }
            Spacer()
            if selectedIsMarkdown, case .text = preview {
                modeBtn(compact ? nil : "Preview", "doc.richtext", on: !markdownRaw) { setMarkdownRaw(false) }
                modeBtn(compact ? nil : "Raw", "chevron.left.forwardslash.chevron.right", on: markdownRaw) { setMarkdownRaw(true) }
            }
            if dirty {
                Circle().fill(Theme.accent).frame(width: 6, height: 6)
                Button { save() } label: {
                    Text("Save").font(.system(size: 11, weight: .medium)).foregroundStyle(.white)
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(Theme.accent, in: RoundedRectangle(cornerRadius: 6))
                }.buttonStyle(.plain).help("Save (Cmd+S)")
            }
            IconButton("arrow.clockwise", size: 11, tip: "Refresh file list") {
                Task { await reload() }
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 5)
        .background(Theme.bgSoft)
    }

    private func setMarkdownRaw(_ raw: Bool) {
        guard markdownRaw != raw else { return }
        markdownRaw = raw
        selLines = nil
        question = ""
    }

    private func modeBtn(_ label: String?, _ icon: String, on: Bool, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            HStack(spacing: 4) {
                Image(systemName: icon).font(.system(size: 10))
                if let label { Text(label).font(.system(size: 11)) }
            }
            .foregroundStyle(on ? Theme.accent : Theme.fgMuted)
            .padding(.horizontal, label == nil ? 6 : 8).padding(.vertical, 3)
            .background(on ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
        .help(markdownRaw ? "Switch to rendered preview" : "Switch to raw source")
    }

    @ViewBuilder private var content: some View {
        if let sel = selected {
            switch preview {
            case .loading:
                LoadingView(text: "loading…")
            case .text(let s):
                if selectedIsMarkdown && !markdownRaw {
                    ScrollView {
                        MarkdownText(s, reading: true)
                            .padding(.horizontal, 20).padding(.vertical, 16)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                } else {
                    // Always editable; tree-sitter highlighting where a grammar is bundled.
                    FileEditor(text: $editText, path: sel.path, mode: theme.mode, editable: true).id(sel.id)
                }
            case .image(let img):
                FileImageView(image: img)
            case .unsupported(let mime):
                EmptyStateView(icon: "doc.questionmark", title: "Can't preview this file", subtitle: mime)
            case .failed(let msg):
                EmptyStateView(icon: "exclamationmark.triangle", title: msg)
            }
        } else {
            EmptyStateView(icon: "doc.text", title: "Select a file to preview")
        }
    }

    private func askBar(file: WorkspaceFileEntry, lines: ClosedRange<Int>) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "text.viewfinder").font(.system(size: 10)).foregroundStyle(Theme.accent)
            Text("L\(lines.lowerBound)-\(lines.upperBound)")
                .font(Theme.mono(10.5)).foregroundStyle(Theme.fgMuted).fixedSize()
            TextField("Ask Claude about these lines…", text: $question)
                .textFieldStyle(.plain).font(.system(size: 12))
                .onSubmit { ask(file: file, lines: lines) }
            Button { ask(file: file, lines: lines) } label: {
                HStack(spacing: 4) {
                    Image(systemName: "sparkles").font(.system(size: 10))
                    Text("Ask").font(.system(size: 12, weight: .medium))
                }
                .foregroundStyle(.white).padding(.horizontal, 12).padding(.vertical, 4)
                .background(Theme.accent, in: RoundedRectangle(cornerRadius: 7))
            }
            .buttonStyle(.plain)
            .disabled(question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            IconButton("xmark", size: 11, tip: "Dismiss") {
                withAnimation(.easeInOut(duration: 0.12)) { selLines = nil; question = "" }
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .background(Theme.bgSoft)
    }

    private func ask(file: WorkspaceFileEntry, lines: ClosedRange<Int>) {
        let q = question.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return }
        onAskAgent("[\(file.repo)/\(file.path):\(lines.lowerBound)-\(lines.upperBound)] \(q)")
        question = ""
    }

    private func startWatch() {
        guard streamID == 0 else { return }
        streamID = StreamManager.shared.openFiles(branch: workspace.branch, isMain: workspace.isMain) { kind, bytes in
            guard kind == .json, !bytes.isEmpty else { return }
            Task { await apply(Data(bytes)) }
        }
        if streamID <= 0 {
            streamID = 0
            Task { await reload() }
        }
    }

    private func stopWatch() {
        guard streamID > 0 else { return }
        StreamManager.shared.close(streamID)
        streamID = 0
    }

    private func reload() async {
        let branch = workspace.branch, isMain = workspace.isMain
        let raw = await Task.detached(priority: .userInitiated) {
            FileStore.list(branch: branch, isMain: isMain)
        }.value
        await apply(raw)
    }

    private func apply(_ raw: Data) async {
        let rootName = (workspace.path as NSString).lastPathComponent
        let built = await Task.detached(priority: .userInitiated) { () -> ([WorkspaceFileEntry], [WFileTreeNode]) in
            let list = PomJSON.decode([WorkspaceFileEntry].self, from: raw) ?? []
            return (list, WFileTreeBuilder.build(list, rootName: rootName))
        }.value
        entries = built.0
        roots = built.1
        treeVersion &+= 1
        expanded.insert("")   // keep the workspace-root node open by default
        if let sel = selected, !built.0.contains(where: { $0.id == sel.id }) {
            selected = nil
        }
    }

    private func loadPreview() async {
        guard let sel = selected else { return }
        preview = .loading
        let branch = workspace.branch, isMain = workspace.isMain
        let resp = await Task.detached(priority: .userInitiated) { () -> FileContentResponse? in
            PomJSON.decode(FileContentResponse.self, from: FileStore.read(branch: branch, repo: sel.repo, path: sel.path, isMain: isMain))
        }.value
        guard let resp, resp.error == nil else {
            preview = .failed(resp?.error ?? "not found")
            return
        }
        if let text = resp.text {
            preview = .text(text)
            savedText = text; editText = text
        } else if let b64 = resp.base64, let data = Data(base64Encoded: b64), let img = NSImage(data: data) {
            preview = .image(img)
        } else if resp.binary {
            preview = .unsupported(resp.mimeType)
        } else {
            preview = .failed("not found")
        }
    }
}
