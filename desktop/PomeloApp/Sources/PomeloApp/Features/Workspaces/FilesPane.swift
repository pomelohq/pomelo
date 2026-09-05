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
    @ObservedObject private var codeDisplay = CodeDisplayManager.shared

    @State private var entries: [WorkspaceFileEntry]?
    @State private var roots: [WFileTreeNode] = []
    @State private var selected: WorkspaceFileEntry?
    @State private var preview: FilePreview = .loading
    @State private var treeVisible = true
    @State private var selLines: ClosedRange<Int>?
    @State private var question = ""

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
        .task { await load() }
        .task(id: selected?.id) { selLines = nil; question = ""; await loadPreview() }
    }

    private var tree: some View {
        Group {
            if let entries {
                if entries.isEmpty {
                    EmptyStateView(icon: "folder", title: "No files")
                } else {
                    WorkspaceFileTreeList(roots: roots, workspacePath: workspace.path, selected: $selected)
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
                Text(selected.repo).font(Theme.mono(11, .semibold)).foregroundStyle(Theme.accent)
                Text(selected.path).font(Theme.mono(11)).foregroundStyle(Theme.fg)
                    .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
            } else {
                Text("Select a file").font(.system(size: 12)).foregroundStyle(Theme.dim)
            }
            Spacer()
        }
        .padding(.horizontal, 10).padding(.vertical, 5)
        .background(Theme.bgSoft)
    }

    @ViewBuilder private var content: some View {
        if let sel = selected {
            switch preview {
            case .loading:
                LoadingView(text: "loading…")
            case .text(let s):
                VStack(spacing: 0) {
                    CodeView(content: s, lang: CodeLang.detect(path: sel.path),
                             start: 0, end: 0, isDark: theme.mode.isDark, wrapMode: codeDisplay.wrapMode,
                             onSelectLines: { sel in
                                 withAnimation(.easeInOut(duration: 0.12)) { selLines = sel }
                             })
                    if let lines = selLines {
                        Divider().overlay(Theme.borderSoft)
                        askBar(file: sel, lines: lines)
                    }
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

    private func load() async {
        let branch = workspace.branch, isMain = workspace.isMain
        let built = await Task.detached(priority: .userInitiated) { () -> ([WorkspaceFileEntry], [WFileTreeNode]) in
            let list = PomJSON.decode([WorkspaceFileEntry].self, from: FileStore.list(branch: branch, isMain: isMain)) ?? []
            return (list, WFileTreeBuilder.build(list))
        }.value
        entries = built.0
        roots = built.1
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
        } else if let b64 = resp.base64, let data = Data(base64Encoded: b64), let img = NSImage(data: data) {
            preview = .image(img)
        } else if resp.binary {
            preview = .unsupported(resp.mimeType)
        } else {
            preview = .failed("not found")
        }
    }
}
