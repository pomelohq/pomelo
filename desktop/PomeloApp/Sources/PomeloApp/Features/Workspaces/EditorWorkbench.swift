import SwiftUI
import UniformTypeIdentifiers

private struct EmptyPaneWatcher: View {
    let wb: Workbench
    let onEmpty: () -> Void
    var body: some View {
        Color.clear
            .onChange(of: wb.tabs.isEmpty) { _, empty in if empty { onEmpty() } }
    }
}

@MainActor @Observable final class PaneChrome {
    var focusedLeaf = "leaf-0"
    var hoverTab: String?
    var draggingTab = false
}

enum SplitDir {
    case right, left, down, up
    var isVertical: Bool { self == .down || self == .up }
    var splitFirst: Bool { self == .left || self == .up }
    var axis: PaneNode.Axis { isVertical ? .vertical : .horizontal }
}

struct EditorWorkbench: View {
    @EnvironmentObject var theme: ThemeManager
    @Environment(AppState.self) private var appState
    let workspace: Workspace
    @Bindable var workbench: Workbench
    var onAskAgent: (String) -> Void = { _ in }
    var onOpenInTerminal: (String) -> Void = { _ in }
    @Binding var openRequest: WorkspaceFileEntry?
    @Binding var searchRequest: Bool

    @State private var root: PaneNode?
    @State private var nextLeafSeq = 1
    @State private var dragMonitor: Any?
    @State private var lastTap: (id: String, at: Date)?
    @State private var chrome = PaneChrome()
    @AppStorage("editorTreeWidth") private var treeWidth = 240.0

    private var focusedLeaf: String { get { chrome.focusedLeaf } nonmutating set { chrome.focusedLeaf = newValue } }
    private var hoverTab: String? { get { chrome.hoverTab } nonmutating set { chrome.hoverTab = newValue } }
    private var draggingTab: Bool { get { chrome.draggingTab } nonmutating set { chrome.draggingTab = newValue } }

    private var focusWb: Workbench { root?.leaf(withID: focusedLeaf)?.leafWorkbench ?? workbench }
    private var leafCount: Int { root?.leaves.count ?? 1 }

    var body: some View {
        HStack(spacing: 0) {
            PanelDock(edge: .left, isOpen: workbench.treeVisible, size: $treeWidth, minSize: 160, maxSize: 480) {
                sidebar
            }
            paneCanvas
        }
        .background(Theme.bg)
        .onAppear {
            if root == nil {
                let restored = restoreTree()
                root = restored
                focusedLeaf = restored.leaves.first?.id ?? "leaf-0"
            }
        }
        .task(id: workspace.id) {
            await workbench.loadTree(branch: workspace.branch, isMain: workspace.isMain, path: workspace.path)
        }
    }

    private var treeKey: String { "workbench.tree.\(workspace.path)" }

    private func saveTree() {
        guard let root, let data = try? JSONEncoder().encode(root.encoded()) else { return }
        UserDefaults.standard.set(data, forKey: treeKey)
    }

    private func restoreTree() -> PaneNode {
        guard let data = UserDefaults.standard.data(forKey: treeKey),
              let snapshot = try? JSONDecoder().decode(PersistedPane.self, from: data) else {
            return .leaf(workbench, id: "leaf-0")
        }
        let node = PaneNode.decoded(snapshot) { leafID in
            if leafID == "leaf-0" { return workbench }
            let w = Workbench(); w.treeVisible = workbench.treeVisible; return w
        }
        nextLeafSeq = node.maxSeq + 1
        return node
    }

    @ViewBuilder private var paneCanvas: some View {
        if let root {
            PaneContainerHost(
                root: root,
                structureSignature: root.structureSignature,
                primaryToken: "\(openRequest?.id ?? "-")|\(searchRequest)",
                focusedLeaf: chrome.focusedLeaf,
                dragActive: chrome.draggingTab,
                leafContent: { node in AnyView(paneContent(node)) },
                onFraction: { saveTree() })
        } else {
            Color.clear
        }
    }

    @ViewBuilder private func paneContent(_ node: PaneNode) -> some View {
        if let wb = node.leafWorkbench { pane(wb, leafID: node.id) }
    }

    private func pane(_ wb: Workbench, leafID: String) -> some View {
        let focused = focusedLeaf == leafID
        return VStack(spacing: 0) {
            tabStrip(wb, leafID: leafID, focused: leafCount > 1 && focused)
            Divider().overlay(Theme.borderSoft)
            FilesPane(workspace: workspace, onAskAgent: onAskAgent, onOpenInTerminal: onOpenInTerminal,
                      openRequest: leafID == "leaf-0" ? $openRequest : .constant(nil),
                      searchRequest: leafID == "leaf-0" ? $searchRequest : .constant(false),
                      workbench: wb, persists: true, paneKey: leafID == "leaf-0" ? "" : leafID)
            .overlay {
                if draggingTab {
                    LeafDropCatcher { payload, dir in performLeafDrop(payload, destLeaf: leafID, destWb: wb, dir: dir) }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Rectangle())
        .onTapGesture { focusedLeaf = leafID }
        .background(EmptyPaneWatcher(wb: wb) {
            if leafCount > 1 { closeLeaf(leafID) }
        })
    }

    private func startTabDrag() {
        draggingTab = true
        if let m = dragMonitor { NSEvent.removeMonitor(m) }
        dragMonitor = NSEvent.addLocalMonitorForEvents(matching: .leftMouseUp) { event in endTabDrag(); return event }
    }

    private func endTabDrag() {
        draggingTab = false
        if let m = dragMonitor { NSEvent.removeMonitor(m); dragMonitor = nil }
    }

    private func performLeafDrop(_ payload: String, destLeaf: String, destWb: Workbench, dir: SplitDir?) {
        endTabDrag()
        let parts = payload.components(separatedBy: "\u{1}")
        guard parts.count == 3 else { return }
        let srcLeaf = parts[0], repo = parts[1], path = parts[2]
        let movingID = "file:\(repo)/\(path)"
        if let dir {
            splitWithDrop(destLeaf: destLeaf, dir: dir, srcLeaf: srcLeaf, repo: repo, path: path)
        } else if srcLeaf != destLeaf {
            let srcWb = root?.leaf(withID: srcLeaf)?.leafWorkbench
            destWb.openPermanentCmd = WorkspaceFileEntry(repo: repo, path: path, isDir: false)
            srcWb?.closeCmd = movingID
            focusedLeaf = destLeaf
        }
    }

    private func splitWithDrop(destLeaf: String, dir: SplitDir, srcLeaf: String, repo: String, path: String) {
        guard let root, root.leaf(withID: destLeaf) != nil else { return }
        let entry = WorkspaceFileEntry(repo: repo, path: path, isDir: false)
        root.leaf(withID: srcLeaf)?.leafWorkbench?.closeCmd = "file:\(repo)/\(path)"
        let newID = "leaf-\(nextLeafSeq)"; let branchID = "branch-\(nextLeafSeq)"; nextLeafSeq += 1
        let newWb = Workbench(); newWb.treeVisible = workbench.treeVisible
        FilesPane.seedPersistedTab(workspacePath: workspace.path, paneKey: newID, repo: entry.repo, path: entry.path)
        self.root = root.splitting(leafID: destLeaf, axis: dir.axis, newLeaf: .leaf(newWb, id: newID),
                                   newFirst: dir.splitFirst, branchID: branchID)
        focusedLeaf = newID
        saveTree()
    }

    private var sidebar: some View {
        WorkspaceFileTreeList(
            roots: workbench.treeRoots, workspacePath: workspace.path, treeVersion: workbench.treeVersion,
            onOpenInTerminal: onOpenInTerminal, dirtyKeys: workbench.treeDirty,
            onOpen: { focusWb.openCmd = $0 },
            onActivate: { focusWb.openPermanentCmd = $0 },
            scrollToID: focusWb.activeFile?.id,
            selected: Binding(get: { focusWb.activeFile }, set: { _ in }),
            expanded: $workbench.treeExpanded)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .id(workbench.treeVersion)
    }

    private func tabStrip(_ wb: Workbench, leafID: String, focused: Bool) -> some View {
        VStack(spacing: 0) {
            if focused { Rectangle().fill(Theme.accent).frame(height: 2) }
            HStack(spacing: 0) {
                navButton("chevron.left", enabled: wb.canBack, tip: "Go Back") { focusedLeaf = leafID; wb.backCmd &+= 1 }
                navButton("chevron.right", enabled: wb.canForward, tip: "Go Forward") { focusedLeaf = leafID; wb.forwardCmd &+= 1 }
                Rectangle().fill(Theme.borderSoft).frame(width: 1, height: 20)
                ScrollViewReader { proxy in
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 0) { ForEach(wb.tabs) { tabView(wb, leafID: leafID, tab: $0).id($0.id) } }
                    }
                    .onChange(of: wb.active) { _, id in
                        guard let id else { return }
                        withAnimation(.easeOut(duration: 0.18)) { proxy.scrollTo(id) }
                    }
                    .onChange(of: wb.tabs.count) { _, _ in
                        guard let id = wb.active else { return }
                        withAnimation(.easeOut(duration: 0.18)) { proxy.scrollTo(id) }
                    }
                }
                Rectangle().fill(Theme.borderSoft).frame(width: 1, height: 20)
                newMenu(wb, leafID: leafID)
                splitMenu(leafID)
                barButton(workbench.zoomed ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right",
                          tip: workbench.zoomed ? "Zoom Out" : "Zoom In", active: workbench.zoomed) { toggleZoom() }
            }
        }
        .frame(height: 34)
        .background(Theme.bgSoft)
    }

    private func tabClicked(_ wb: Workbench, leafID: String, _ id: String) {
        focusedLeaf = leafID
        let now = Date()
        if let last = lastTap, last.id == id, now.timeIntervalSince(last.at) < 0.35 {
            wb.promoteCmd = id
            lastTap = nil
        } else {
            wb.activateCmd = id
            lastTap = (id, now)
        }
    }

    private func showTabMenu(_ wb: Workbench, tab: WorkTab, at screen: NSPoint) {
        let file: (repo: String, path: String)? = { if case .file(let r, let p) = tab.kind { return (r, p) }; return nil }()
        ContextMenu.show(at: screen) { _ in
            ContextMenu.container {
                ContextMenuRow(label: "Close") { wb.closeCmd = tab.id }
                ContextMenuRow(label: "Close Others") { wb.tabClose(.others, tab.id) }
                ContextMenuRow(label: "Close to the Right") { wb.tabClose(.right, tab.id) }
                ContextMenuRow(label: "Close to the Left") { wb.tabClose(.left, tab.id) }
                ContextMenuRow(label: "Close All") { wb.tabClose(.all, tab.id) }
                if let file {
                    let abs = (workspace.path as NSString).appendingPathComponent(file.repo.isEmpty ? file.path : file.repo + "/" + file.path)
                    ContextMenuSeparator()
                    ContextMenuRow(label: "Copy Path", symbol: "doc.on.clipboard") {
                        NSPasteboard.general.clearContents(); NSPasteboard.general.setString(abs, forType: .string)
                    }
                    ContextMenuRow(label: "Reveal in Finder", symbol: "magnifyingglass") {
                        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: abs)])
                    }
                }
            }
        }
    }

    private func tabView(_ wb: Workbench, leafID: String, tab: WorkTab) -> some View {
        let active = wb.active == tab.id
        return HStack(spacing: 6) {
            if let mi = tab.materialIcon {
                Image(mi, bundle: .module).resizable().interpolation(.high).frame(width: 14, height: 14)
            } else {
                Image(systemName: tab.systemIcon).font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            }
            Text(tab.label).font(.system(size: 12)).italic(tab.preview).lineLimit(1).fixedSize()
            Group {
                if tab.dirty && hoverTab != tab.id {
                    Circle().fill(active ? Theme.fg : Theme.fgMuted).frame(width: 6, height: 6)
                } else if active || hoverTab == tab.id {
                    Button { wb.closeCmd = tab.id } label: {
                        Image(systemName: "xmark").font(.system(size: 9))
                    }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
                } else {
                    Color.clear.frame(width: 6)
                }
            }.frame(width: 14)
        }
        .foregroundStyle(active ? Theme.fg : Theme.fgMuted)
        .padding(.horizontal, 10)
        .frame(maxHeight: .infinity)
        .background(active ? Theme.bg : (hoverTab == tab.id ? Theme.hover : .clear))
        .overlay(alignment: .trailing) { Rectangle().fill(Theme.borderSoft).frame(width: 1) }
        .contentShape(Rectangle())
        .overlay { RightClickArea { screen in focusedLeaf = leafID; showTabMenu(wb, tab: tab, at: screen) } }
        .onTapGesture { tabClicked(wb, leafID: leafID, tab.id) }
        .onHover { hoverTab = $0 ? tab.id : (hoverTab == tab.id ? nil : hoverTab) }
        .ifLet(filePayload(leafID: leafID, tab: tab)) { view, payload in
            view.onDrag { startTabDrag(); return NSItemProvider(object: payload as NSString) }
        }
        .dropDestination(for: String.self) { items, _ in
            endTabDrag()
            guard let payload = items.first else { return false }
            handleDropOnTab(payload, destLeaf: leafID, destWb: wb, targetTab: tab)
            return true
        }
    }

    private func newMenu(_ wb: Workbench, leafID: String) -> some View {
        Menu {
            Button("Open File") { focusedLeaf = leafID; NotificationCenter.default.post(name: .pomQuickOpen, object: nil) }
            Button("Search in Project") { focusedLeaf = "leaf-0"; searchRequest = true }
        } label: {
            Image(systemName: "plus").font(.system(size: 11, weight: .medium))
                .foregroundStyle(Theme.fgMuted).frame(width: 30, height: 34).contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
        .help("New...")
    }

    private func splitMenu(_ leafID: String) -> some View {
        Menu {
            Button("Split Right") { openSplit(leafID, .right) }
            Button("Split Left") { openSplit(leafID, .left) }
            Button("Split Up") { openSplit(leafID, .up) }
            Button("Split Down") { openSplit(leafID, .down) }
            if leafCount > 1 {
                Divider()
                Button("Close This Pane") { closeLeaf(leafID) }
            }
        } label: {
            Image(systemName: "rectangle.split.2x1").font(.system(size: 11, weight: .medium))
                .foregroundStyle(leafCount > 1 ? Theme.accent : Theme.fgMuted)
                .frame(width: 30, height: 34).contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
        .help("Split Editor")
    }

    private func openSplit(_ leafID: String, _ dir: SplitDir) {
        guard let root, root.leaf(withID: leafID) != nil else { return }
        let seed = root.leaf(withID: leafID)?.leafWorkbench?.activeFile
        let newID = "leaf-\(nextLeafSeq)"; let branchID = "branch-\(nextLeafSeq)"; nextLeafSeq += 1
        let newWb = Workbench(); newWb.treeVisible = workbench.treeVisible
        if let seed {
            FilesPane.seedPersistedTab(workspacePath: workspace.path, paneKey: newID, repo: seed.repo, path: seed.path)
        }
        self.root = root.splitting(leafID: leafID, axis: dir.axis, newLeaf: .leaf(newWb, id: newID),
                                   newFirst: dir.splitFirst, branchID: branchID)
        focusedLeaf = newID
        saveTree()
    }

    private func closeLeaf(_ leafID: String) {
        guard let r = root else { return }
        guard r.leaves.count > 1 else { return }
        let newRoot = r.removing(leafID: leafID)
        root = newRoot
        if focusedLeaf == leafID { focusedLeaf = newRoot?.leaves.first?.id ?? "leaf-0" }
        saveTree()
    }

    private func toggleZoom() {
        if workbench.zoomed {
            workbench.treeVisible = workbench.preZoomTreeVisible
            appState.sidebarCollapsed = workbench.preZoomSidebarCollapsed
            workbench.zoomed = false
        } else {
            workbench.preZoomTreeVisible = workbench.treeVisible
            workbench.preZoomSidebarCollapsed = appState.sidebarCollapsed
            workbench.treeVisible = false
            appState.sidebarCollapsed = true
            workbench.zoomed = true
        }
    }

    private func navButton(_ icon: String, enabled: Bool, tip: String, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            Image(systemName: icon).font(.system(size: 11, weight: .medium))
                .foregroundStyle(enabled ? Theme.fgMuted : Theme.dim)
                .frame(width: 30, height: 34).contentShape(Rectangle())
        }.buttonStyle(.plain).disabled(!enabled).help(tip)
    }

    private func barButton(_ icon: String, tip: String, active: Bool = false, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            Image(systemName: icon).font(.system(size: 11, weight: .medium))
                .foregroundStyle(active ? Theme.accent : Theme.fgMuted)
                .frame(width: 30, height: 34).contentShape(Rectangle())
        }.buttonStyle(.plain).help(tip)
    }

    private func filePayload(leafID: String, tab: WorkTab) -> String? {
        guard case .file(let repo, let path) = tab.kind else { return nil }
        return "\(leafID)\u{1}\(repo)\u{1}\(path)"
    }

    private func handleDropOnTab(_ payload: String, destLeaf: String, destWb: Workbench, targetTab: WorkTab) {
        let parts = payload.components(separatedBy: "\u{1}")
        guard parts.count == 3 else { return }
        let srcLeaf = parts[0], repo = parts[1], path = parts[2]
        let movingID = "file:\(repo)/\(path)"
        if srcLeaf == destLeaf {
            guard movingID != targetTab.id else { return }
            destWb.moveTab = TabMove(moving: movingID, target: targetTab.id, seq: (destWb.moveTab?.seq ?? 0) + 1)
        } else {
            let srcWb = root?.leaf(withID: srcLeaf)?.leafWorkbench
            destWb.openPermanentCmd = WorkspaceFileEntry(repo: repo, path: path, isDir: false)
            srcWb?.closeCmd = movingID
            focusedLeaf = destLeaf
        }
    }
}

private extension View {
    @ViewBuilder func ifLet<V>(_ value: V?, _ transform: (Self, V) -> some View) -> some View {
        if let value { transform(self, value) } else { self }
    }
}

private struct LeafDropCatcher: View {
    let onPerform: (String, SplitDir?) -> Void
    @State private var hover: LeafHover?

    struct LeafHover: Equatable { var dir: SplitDir? }

    var body: some View {
        GeometryReader { geo in
            ZStack {
                if let h = hover {
                    let r = LeafDrop.overlayRect(h.dir, geo.size)
                    Rectangle().fill(Theme.accent.opacity(0.16))
                        .overlay(Rectangle().stroke(Theme.accent.opacity(0.6), lineWidth: 2))
                        .frame(width: r.width, height: r.height)
                        .position(x: r.midX, y: r.midY)
                        .allowsHitTesting(false)
                }
                Color.clear.contentShape(Rectangle())
                    .onDrop(of: [.utf8PlainText, .plainText, .text], delegate: LeafDrop(
                        size: { geo.size },
                        onUpdate: { h in if hover != h { hover = h } },
                        onPerform: { payload, dir in hover = nil; onPerform(payload, dir) }))
            }
        }
    }
}

private struct LeafDrop: DropDelegate {
    let size: () -> CGSize
    let onUpdate: (LeafDropCatcher.LeafHover?) -> Void
    let onPerform: (String, SplitDir?) -> Void

    func dropEntered(info: DropInfo) { onUpdate(.init(dir: Self.zone(info.location, size()))) }
    func dropUpdated(info: DropInfo) -> DropProposal? {
        onUpdate(.init(dir: Self.zone(info.location, size())))
        return DropProposal(operation: .move)
    }
    func dropExited(info: DropInfo) { onUpdate(nil) }

    func performDrop(info: DropInfo) -> Bool {
        let dir = Self.zone(info.location, size())
        guard let provider = info.itemProviders(for: [.utf8PlainText, .plainText, .text]).first else { return false }
        _ = provider.loadObject(ofClass: NSString.self) { obj, _ in
            guard let s = obj as? String else { return }
            DispatchQueue.main.async { onPerform(s, dir) }
        }
        return true
    }

    static func zone(_ p: CGPoint, _ size: CGSize) -> SplitDir? {
        guard size.width > 0, size.height > 0 else { return nil }
        let band = min(size.width, size.height) * 0.25
        let inEdge = p.x < band || p.x > size.width - band || p.y < band || p.y > size.height - band
        guard inEdge else { return nil }
        let dist: [(SplitDir, CGFloat)] = [(.up, p.y), (.right, size.width - p.x), (.down, size.height - p.y), (.left, p.x)]
        return dist.min { $0.1 < $1.1 }?.0
    }

    static func overlayRect(_ dir: SplitDir?, _ s: CGSize) -> CGRect {
        switch dir {
        case .none: return CGRect(x: 0, y: 0, width: s.width, height: s.height)
        case .up: return CGRect(x: 0, y: 0, width: s.width, height: s.height / 2)
        case .down: return CGRect(x: 0, y: s.height / 2, width: s.width, height: s.height / 2)
        case .left: return CGRect(x: 0, y: 0, width: s.width / 2, height: s.height)
        case .right: return CGRect(x: s.width / 2, y: 0, width: s.width / 2, height: s.height)
        }
    }
}
