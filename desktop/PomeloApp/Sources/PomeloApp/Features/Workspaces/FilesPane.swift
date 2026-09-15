import SwiftUI
import AppKit
import CodeEditSourceEditor

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

// One open editor tab. `preview` tabs render italic and are reused by the next single-click open (Zed/VSCode style);
// editing or double-clicking promotes them to permanent.
private struct FileTab: Identifiable, Equatable {
    var entry: WorkspaceFileEntry
    var preview: Bool
    var id: String { entry.id }
}

struct FilesPane: View {
    @EnvironmentObject var theme: ThemeManager
    let workspace: Workspace
    var onAskAgent: (String) -> Void = { _ in }
    var onOpenInTerminal: (String) -> Void = { _ in }
    @Binding var openRequest: WorkspaceFileEntry?
    @Binding var searchRequest: Bool

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
    @State private var didRestore = false
    @State private var dirtyKeys: Set<String> = []
    @State private var changedLines: [Int: Int] = [:]
    @State private var blameLines: [Int: String] = [:]
    @State private var editorState = SourceEditorState()
    @Environment(AppState.self) private var appState
    @AppStorage("fileEditorFontSize") private var fontSize: Double = 12

    // Tab model. `selected` is the active tab's file; opens route through `open(_:preview:)`.
    @State private var tabs: [FileTab] = []
    @State private var buffers: [String: (edit: String, saved: String)] = [:]
    @State private var history: [String] = []
    @State private var histIdx = -1
    @State private var hoverTab: String?
    @State private var suppressHistory = false
    @State private var lastTabClick: (id: String, at: Date)?
    // Project-search (Cmd+Shift+F) lives as its own tab, Zed-style.
    @State private var searchOpen = false
    @State private var searchActive = false
    @State private var searchQuery = ""
    @State private var pendingJump: Int?

    private var selectedIsMarkdown: Bool {
        guard let path = selected?.path else { return false }
        let ext = (path as NSString).pathExtension.lowercased()
        return ext == "md" || ext == "markdown"
    }

    private var isTextPreview: Bool { if case .text = preview { return true }; return false }
    private var dirty: Bool { isTextPreview && editText != savedText }

    // MARK: - Tabs

    private var activeID: String? { selected?.id }

    /// A tab is dirty if its cached buffer has unsaved edits (or, for the active tab, the live editor differs).
    private func tabDirty(_ id: String) -> Bool {
        if id == activeID { return dirty }
        if let b = buffers[id] { return b.edit != b.saved }
        return false
    }

    /// The single entry point for opening a file. Reuses an existing tab, replaces the current preview tab, or
    /// appends a new one. `preview` tabs are transient (italic) until edited or double-clicked.
    private func open(_ e: WorkspaceFileEntry, preview: Bool) {
        if let idx = tabs.firstIndex(where: { $0.id == e.id }) {
            if !preview { tabs[idx].preview = false }
        } else if preview, let pIdx = tabs.firstIndex(where: { $0.preview }) {
            tabs[pIdx] = FileTab(entry: e, preview: true)
        } else {
            tabs.append(FileTab(entry: e, preview: preview))
        }
        searchActive = false
        revealInTree(e)
        selected = e
        pushHistory(e.id)
        if didRestore { saveState() }
    }

    private func activate(_ id: String) {
        guard let t = tabs.first(where: { $0.id == id }) else { return }
        searchActive = false
        selected = t.entry
        revealInTree(t.entry)
        pushHistory(id)
        if didRestore { saveState() }
    }

    private func promoteActive() {
        guard let id = activeID, let idx = tabs.firstIndex(where: { $0.id == id }), tabs[idx].preview else { return }
        tabs[idx].preview = false
        if didRestore { saveState() }
    }

    private func openSearch() { searchOpen = true; searchActive = true; if didRestore { saveState() } }
    private func closeSearch() { searchOpen = false; searchActive = false; if didRestore { saveState() } }

    private func close(_ id: String) {
        guard let idx = tabs.firstIndex(where: { $0.id == id }) else { return }
        let wasActive = id == activeID
        tabs.remove(at: idx)
        buffers[id] = nil
        history.removeAll { $0 == id }
        histIdx = min(histIdx, history.count - 1)
        if wasActive {
            let next = tabs.indices.contains(idx) ? tabs[idx] : tabs.last
            selected = next?.entry
        }
        if didRestore { saveState() }
    }

    private func closeOthers(_ id: String) {
        for t in tabs where t.id != id { buffers[t.id] = nil }
        tabs.removeAll { $0.id != id }
        history.removeAll { $0 != id }; histIdx = history.count - 1
        if let keep = tabs.first { selected = keep.entry }
        if didRestore { saveState() }
    }

    private func closeAll() {
        tabs.removeAll(); buffers.removeAll(); history.removeAll(); histIdx = -1
        selected = nil
        if didRestore { saveState() }
    }

    private func closeToRight(_ id: String) {
        guard let idx = tabs.firstIndex(where: { $0.id == id }) else { return }
        let removed = tabs[(idx + 1)...].map(\.id)
        tabs.removeSubrange((idx + 1)...)
        for r in removed { buffers[r] = nil; history.removeAll { $0 == r } }
        histIdx = min(histIdx, history.count - 1)
        if let sel = selected, removed.contains(sel.id) { selected = tabs[idx].entry }
        if didRestore { saveState() }
    }

    // Per-pane back/forward history of visited tabs.
    private func pushHistory(_ id: String) {
        if suppressHistory { return }
        if histIdx >= 0, histIdx < history.count, history[histIdx] == id { return }
        if histIdx < history.count - 1 { history.removeSubrange((histIdx + 1)...) }
        history.append(id)
        histIdx = history.count - 1
    }

    private var canBack: Bool { histIdx > 0 }
    private var canForward: Bool { histIdx >= 0 && histIdx < history.count - 1 }

    private func goBack() {
        guard canBack else { return }
        histIdx -= 1
        navigateHistory()
    }

    private func goForward() {
        guard canForward else { return }
        histIdx += 1
        navigateHistory()
    }

    private func navigateHistory() {
        guard histIdx >= 0, histIdx < history.count,
              let t = tabs.first(where: { $0.id == history[histIdx] }) else { return }
        suppressHistory = true
        selected = t.entry
        revealInTree(t.entry)
        suppressHistory = false
    }

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
                    if !tabs.isEmpty || searchOpen {
                        tabBar
                        Divider().overlay(Theme.borderSoft)
                    }
                    ZStack {
                        VStack(spacing: 0) {
                            topBar(compact: overlayTree)
                            Divider().overlay(Theme.borderSoft)
                            // Clip so the editor's gutter can't overdraw upward into the path bar.
                            content.clipped()
                        }
                        .opacity(searchActive ? 0 : 1)
                        .allowsHitTesting(!searchActive)
                        // Keep the search view mounted while its tab is open so results survive switching tabs.
                        if searchOpen {
                            FindInFiles(branch: workspace.branch, isMain: workspace.isMain, mode: theme.mode,
                                        workspacePath: workspace.path,
                                        query: $searchQuery,
                                        onChoose: { e, line in searchActive = false; pendingJump = line; open(e, preview: false) },
                                        onClose: { closeSearch() })
                                .opacity(searchActive ? 1 : 0)
                                .allowsHitTesting(searchActive)
                        }
                    }
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
            Group {
                Button("") { save() }.keyboardShortcut("s", modifiers: .command)
                Button("") { fontSize = min(fontSize + 1, 28) }.keyboardShortcut("=", modifiers: .command)
                Button("") { fontSize = min(fontSize + 1, 28) }.keyboardShortcut("+", modifiers: .command)
                Button("") { fontSize = max(fontSize - 1, 8) }.keyboardShortcut("-", modifiers: .command)
                Button("") { fontSize = 12 }.keyboardShortcut("0", modifiers: .command)
            }
            .opacity(0).allowsHitTesting(false)
        }
        .onAppear { startWatch(); if searchRequest { openSearch(); searchRequest = false } }
        .onChange(of: expanded) { _ in if didRestore { saveState() } }
        .onChange(of: searchQuery) { _, _ in if didRestore { saveState() } }
        .onChange(of: selected) { old, _ in
            // Stash the outgoing tab's live buffer so its unsaved edits survive a tab switch.
            if let o = old, tabs.contains(where: { $0.id == o.id }) { buffers[o.id] = (editText, savedText) }
            if didRestore { saveState() }
        }
        .onChange(of: editText) { _, _ in if editText != savedText { promoteActive() } }
        .onDisappear { stopWatch() }
        .task(id: selected?.id) {
            selLines = nil; question = ""
            await loadPreview(); await refreshDiff(); await refreshBlame()
            if let l = pendingJump {
                editorState.cursorPositions = [CursorPosition(line: l, column: 1)]
                pendingJump = nil
            }
        }
        .onChange(of: openRequest) { _, req in
            guard let e = req else { return }
            open(e, preview: true)   // Cmd+P quick-open = preview tab
            openRequest = nil
        }
        .onReceive(NotificationCenter.default.publisher(for: .pomFindInFile)) { _ in
            guard appState.selectedWorkspace?.id == workspace.id, !searchActive, selected != nil else { return }
            editorState.findPanelVisible = true
        }
        .onChange(of: searchRequest) { _, req in if req { openSearch(); searchRequest = false } }
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

    private func editorAbsPath(_ e: WorkspaceFileEntry) -> String {
        let rel = e.repo.isEmpty ? e.path : e.repo + "/" + e.path
        return (workspace.path as NSString).appendingPathComponent(rel)
    }

    private func showEditorMenu(_ sel: WorkspaceFileEntry, at point: NSPoint, in view: NSView) {
        ContextMenu.show(inView: view, at: point) { _ in
            ContextMenu.container {
                ContextMenuRow(label: "Cut", symbol: "scissors") { NSApp.sendAction(Selector(("cut:")), to: nil, from: nil) }
                ContextMenuRow(label: "Copy", symbol: "doc.on.doc") { NSApp.sendAction(Selector(("copy:")), to: nil, from: nil) }
                ContextMenuRow(label: "Paste", symbol: "clipboard") { NSApp.sendAction(Selector(("paste:")), to: nil, from: nil) }
                ContextMenuSeparator()
                ContextMenuRow(label: "Copy Path", symbol: "doc.on.clipboard") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(editorAbsPath(sel), forType: .string)
                }
                ContextMenuRow(label: "Copy Relative Path") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(sel.repo.isEmpty ? sel.path : sel.repo + "/" + sel.path, forType: .string)
                }
                ContextMenuSeparator()
                ContextMenuRow(label: "Reveal in Finder", symbol: "magnifyingglass") {
                    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: editorAbsPath(sel))])
                }
                ContextMenuRow(label: "Open in Terminal", symbol: "terminal") {
                    onOpenInTerminal((editorAbsPath(sel) as NSString).deletingLastPathComponent)
                }
            }
        }
    }

    private func save() {
        guard dirty, let sel = selected else { return }
        let rel = sel.repo.isEmpty ? sel.path : sel.repo + "/" + sel.path
        let abs = (workspace.path as NSString).appendingPathComponent(rel)
        do {
            try editText.write(toFile: abs, atomically: true, encoding: .utf8)
            savedText = editText
            Task { await refreshDiff(); await refreshBlame(); await refreshGitStatus() }
        } catch {
            let alert = NSAlert(); alert.messageText = "Couldn't save"
            alert.informativeText = error.localizedDescription
            alert.alertStyle = .warning; alert.addButton(withTitle: "OK"); alert.runModal()
        }
    }

    // Change-gutter markers for the open file: 0-based line -> kind (1 added, 2 modified, 3 deleted).
    private func refreshDiff() async {
        guard let sel = selected else { changedLines = [:]; return }
        let branch = workspace.branch, isMain = workspace.isMain
        let repo = sel.repo, path = sel.path
        changedLines = await Task.detached(priority: .utility) { () -> [Int: Int] in
            struct Diff: Decodable { var added: [Int] = []; var modified: [Int] = []; var deleted: [Int] = [] }
            let data = FileStore.gitDiff(branch: branch, repo: repo, path: path, isMain: isMain)
            let d = PomJSON.decode(Diff.self, from: data) ?? Diff()
            var m: [Int: Int] = [:]
            for line in d.added { m[line - 1] = 1 }
            for line in d.modified { m[line - 1] = 2 }
            for line in d.deleted { m[line - 1] = 3 }
            return m
        }.value
    }

    // Inline blame for the open file: 0-based line -> "Author, relative date".
    private func refreshBlame() async {
        guard let sel = selected else { blameLines = [:]; return }
        let branch = workspace.branch, isMain = workspace.isMain
        let repo = sel.repo, path = sel.path
        blameLines = await Task.detached(priority: .utility) { () -> [Int: String] in
            struct Line: Decodable { var author = ""; var time: Int64 = 0 }
            struct Payload: Decodable { var lines: [Line] = [] }
            let data = FileStore.gitBlame(branch: branch, repo: repo, path: path, isMain: isMain)
            let payload = PomJSON.decode(Payload.self, from: data) ?? Payload()
            var out: [Int: String] = [:]
            for (i, line) in payload.lines.enumerated() where !line.author.isEmpty {
                out[i] = "\(line.author), \(Self.relativeDate(line.time))"
            }
            return out
        }.value
    }

    private nonisolated static func relativeDate(_ epoch: Int64) -> String {
        guard epoch > 0 else { return "" }
        let secs = max(0, Int(Date().timeIntervalSince1970) - Int(epoch))
        switch secs {
        case ..<60: return "just now"
        case ..<3600: return "\(secs / 60)m ago"
        case ..<86_400: return "\(secs / 3600)h ago"
        case ..<604_800: return "\(secs / 86_400)d ago"
        case ..<2_592_000: return "\(secs / 604_800)w ago"
        case ..<31_536_000: return "\(secs / 2_592_000)mo ago"
        default: return "\(secs / 31_536_000)y ago"
        }
    }

    private var tree: some View {
        Group {
            if let entries {
                if entries.isEmpty {
                    EmptyStateView(icon: "folder", title: "No files")
                } else {
                    WorkspaceFileTreeList(roots: roots, workspacePath: workspace.path, treeVersion: treeVersion,
                                          onOpenInTerminal: onOpenInTerminal, dirtyKeys: dirtyKeys,
                                          onOpen: { open($0, preview: true) },
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

    // MARK: - Tab bar (Zed-style)

    private var tabBar: some View {
        HStack(spacing: 0) {
            HStack(spacing: 1) {
                navBtn("chevron.left", enabled: canBack) { goBack() }
                navBtn("chevron.right", enabled: canForward) { goForward() }
            }
            .padding(.horizontal, 5)
            Rectangle().fill(Theme.borderSoft).frame(width: 1, height: 18)
            ScrollViewReader { proxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 0) {
                        if searchOpen { searchTabView }
                        ForEach(tabs) { tabView($0).id($0.id) }
                    }
                }
                .onChange(of: selected?.id) { _, id in
                    guard let id, !searchActive else { return }
                    withAnimation(.easeInOut(duration: 0.12)) { proxy.scrollTo(id, anchor: .center) }
                }
            }
        }
        .frame(height: 34)
        .background(Theme.bgSoft)
    }

    private func navBtn(_ icon: String, enabled: Bool, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            Image(systemName: icon).font(.system(size: 11, weight: .semibold))
                .foregroundStyle(enabled ? Theme.fgMuted : Theme.dim.opacity(0.4))
                .frame(width: 20, height: 22)
        }
        .buttonStyle(.plain).disabled(!enabled)
    }

    private var searchTabView: some View {
        let active = searchActive
        let hovered = hoverTab == "__search__"
        return HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").font(.system(size: 11))
                .foregroundStyle(active ? Theme.accent : Theme.fgMuted)
            Text("Search").font(.system(size: 12)).foregroundStyle(active ? Theme.fg : Theme.fgMuted)
            ZStack {
                if hovered {
                    Button { closeSearch() } label: {
                        Image(systemName: "xmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.fgMuted)
                            .frame(width: 15, height: 15).background(Theme.sel, in: RoundedRectangle(cornerRadius: 4))
                    }.buttonStyle(.plain)
                }
            }.frame(width: 15, height: 15)
        }
        .padding(.leading, 10).padding(.trailing, 6)
        .frame(height: 34)
        .background(active ? Theme.bg : Theme.bgSoft)
        .overlay(alignment: .top) { if active { Rectangle().fill(Theme.accent).frame(height: 2) } }
        .overlay(alignment: .trailing) { Rectangle().fill(Theme.borderSoft).frame(width: 1) }
        .contentShape(Rectangle())
        .onHover { hoverTab = $0 ? "__search__" : (hoverTab == "__search__" ? nil : hoverTab) }
        .onTapGesture { searchActive = true }
    }

    private func tabView(_ t: FileTab) -> some View {
        let active = !searchActive && t.id == activeID
        let hovered = hoverTab == t.id
        let name = (t.entry.path as NSString).lastPathComponent
        let isDirty = tabDirty(t.id)
        return HStack(spacing: 6) {
            if let mat = MaterialIcon.file(name).map({ "mi-" + $0 }) {
                Image(mat, bundle: .module).resizable().aspectRatio(contentMode: .fit).frame(width: 14, height: 14)
            } else {
                Image(systemName: "doc").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            }
            Text(name).font(.system(size: 12)).italic(t.preview)
                .foregroundStyle(active ? Theme.fg : Theme.fgMuted)
                .lineLimit(1).truncationMode(.middle)
            ZStack {
                if hovered {
                    Button { close(t.id) } label: {
                        Image(systemName: "xmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.fgMuted)
                            .frame(width: 15, height: 15).background(Theme.sel, in: RoundedRectangle(cornerRadius: 4))
                    }.buttonStyle(.plain)
                } else if isDirty {
                    Circle().fill(Theme.fgMuted).frame(width: 7, height: 7)
                }
            }
            .frame(width: 15, height: 15)
        }
        .padding(.leading, 10).padding(.trailing, 6)
        .frame(maxWidth: 200).frame(height: 34)
        .background(active ? Theme.bg : Theme.bgSoft)
        .overlay(alignment: .top) { if active { Rectangle().fill(Theme.accent).frame(height: 2) } }
        .overlay(alignment: .trailing) { Rectangle().fill(Theme.borderSoft).frame(width: 1) }
        .contentShape(Rectangle())
        .onHover { hoverTab = $0 ? t.id : (hoverTab == t.id ? nil : hoverTab) }
        .onTapGesture { tabClicked(t.id) }
        .contextMenu { tabMenu(t) }
        .draggable(t.id) {
            Text(name).font(.system(size: 12)).padding(.horizontal, 8).padding(.vertical, 4)
                .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 6))
        }
        .dropDestination(for: String.self) { items, _ in reorder(dragged: items.first, before: t.id); return true }
    }

    @ViewBuilder private func tabMenu(_ t: FileTab) -> some View {
        Button("Close") { close(t.id) }
        Button("Close Others") { closeOthers(t.id) }.disabled(tabs.count <= 1)
        Button("Close to the Right") { closeToRight(t.id) }.disabled(tabs.last?.id == t.id)
        Button("Close All") { closeAll() }
        Divider()
        Button("Copy Path") { copyString(editorAbsPath(t.entry)) }
        Button("Copy Relative Path") { copyString(t.entry.repo.isEmpty ? t.entry.path : t.entry.repo + "/" + t.entry.path) }
        Button("Reveal in Finder") {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: editorAbsPath(t.entry))])
        }
    }

    private func copyString(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    // Immediate activate on first click; a quick second click on the same tab promotes it to a permanent tab.
    private func tabClicked(_ id: String) {
        let now = Date()
        if let last = lastTabClick, last.id == id, now.timeIntervalSince(last.at) < 0.35 {
            if let idx = tabs.firstIndex(where: { $0.id == id }) { tabs[idx].preview = false; if didRestore { saveState() } }
            lastTabClick = nil
        } else {
            activate(id)
            lastTabClick = (id, now)
        }
    }

    private func reorder(dragged: String?, before targetID: String) {
        guard let dragged, dragged != targetID,
              let from = tabs.firstIndex(where: { $0.id == dragged }) else { return }
        let moved = tabs.remove(at: from)
        let to = tabs.firstIndex(where: { $0.id == targetID }) ?? tabs.count
        tabs.insert(moved, at: to)
        if didRestore { saveState() }
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
                    FileEditor(text: $editText, path: sel.path, mode: theme.mode, editable: true,
                               fontSize: CGFloat(fontSize), changedLines: changedLines, blameLines: blameLines,
                               onRightClick: { pt, view in showEditorMenu(sel, at: pt, in: view) },
                               state: $editorState).id(sel.id)
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
        if !didRestore {
            didRestore = true
            restoreState(from: built.0)
        }
        if let sel = selected, !built.0.contains(where: { $0.id == sel.id }) {
            selected = nil
        }
        await refreshGitStatus()
    }

    // Gold-highlight files with uncommitted git changes and every folder on their path (Zed-style).
    private func refreshGitStatus() async {
        let branch = workspace.branch, isMain = workspace.isMain
        let keys = await Task.detached(priority: .utility) { () -> Set<String> in
            struct Change: Decodable { var path = "" }
            struct Repo: Decodable { var repo = ""; var changes: [Change] = [] }
            struct Payload: Decodable { var repos: [Repo] = [] }
            let data = FileStore.gitStatus(branch: branch, isMain: isMain)
            let payload = PomJSON.decode(Payload.self, from: data) ?? Payload()
            var keys: Set<String> = []
            for repo in payload.repos {
                for change in repo.changes where !change.path.isEmpty {
                    let fileID = repo.repo.isEmpty ? change.path : repo.repo + "/" + change.path
                    keys.insert("")
                    var acc = ""
                    for part in fileID.split(separator: "/") {
                        acc = acc.isEmpty ? String(part) : acc + "/" + part
                        keys.insert(acc)
                    }
                }
            }
            return keys
        }.value
        dirtyKeys = keys
    }

    private struct PersistedTab: Codable { var repo: String; var path: String; var preview: Bool }
    private struct PersistedState: Codable {
        var expanded: [String]
        var selectedRepo: String?
        var selectedPath: String?
        var tabs: [PersistedTab]?
        var searchOpen: Bool?
        var searchQuery: String?
    }

    private var stateKey: String { "filesPane.state.\(workspace.path)" }

    private func saveState() {
        let st = PersistedState(expanded: Array(expanded),
                                selectedRepo: selected?.repo,
                                selectedPath: selected?.path,
                                tabs: tabs.map { PersistedTab(repo: $0.entry.repo, path: $0.entry.path, preview: $0.preview) },
                                searchOpen: searchOpen,
                                searchQuery: searchQuery.isEmpty ? nil : searchQuery)
        if let data = try? JSONEncoder().encode(st) {
            UserDefaults.standard.set(data, forKey: stateKey)
        }
    }

    private func restoreState(from list: [WorkspaceFileEntry]) {
        guard let data = UserDefaults.standard.data(forKey: stateKey),
              let st = try? JSONDecoder().decode(PersistedState.self, from: data) else { return }
        expanded.formUnion(st.expanded)
        if let q = st.searchQuery, !q.isEmpty { searchQuery = q; if st.searchOpen == true { searchOpen = true } }
        if tabs.isEmpty, let saved = st.tabs {
            tabs = saved.compactMap { pt in
                guard let e = list.first(where: { $0.repo == pt.repo && $0.path == pt.path }) else { return nil }
                return FileTab(entry: e, preview: pt.preview)
            }
        }
        guard selected == nil else { return }
        var target = list.first { $0.repo == (st.selectedRepo ?? "") && $0.path == (st.selectedPath ?? "\u{0}") }
        if target == nil { target = tabs.first?.entry }
        if let e = target {
            if !tabs.contains(where: { $0.id == e.id }) { tabs.append(FileTab(entry: e, preview: false)) }
            suppressHistory = true; selected = e; suppressHistory = false
            pushHistory(e.id)
        }
    }

    private func loadPreview() async {
        guard let sel = selected else { return }
        // Restore an in-memory buffer (with any unsaved edits) instead of re-reading from disk.
        if let buf = buffers[sel.id] {
            savedText = buf.saved; editText = buf.edit
            preview = .text(buf.edit)
            return
        }
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
