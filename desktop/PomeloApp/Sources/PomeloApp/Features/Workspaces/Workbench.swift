import SwiftUI

// One tab in the shared workbench strip. Stage 1 covers editor items (files + project search); later stages add
// PR / diff / database tabs from the other panes.
struct WorkTab: Identifiable, Hashable {
    enum Kind: Hashable { case file(repo: String, path: String), search, gitDiff(repo: String, path: String) }
    var kind: Kind
    var label: String
    var materialIcon: String?   // mi-* asset for files; nil -> system icon
    var preview: Bool
    var dirty: Bool

    var id: String {
        switch kind {
        case .file(let repo, let path): return "file:\(repo)/\(path)"
        case .search: return "search"
        case .gitDiff(let repo, let path): return "gitdiff:\(repo)/\(path)"
        }
    }
    var systemIcon: String {
        switch kind {
        case .search: return "magnifyingglass"
        case .file: return "doc.text"
        case .gitDiff: return "plusminus"
        }
    }
}

// Shared state between the WorkspacePane workbench (sidebar + tab strip + content) and the panes that own the real
// content. FilesPane writes `tabs`/`active`/tree state up and reads the `*Cmd` fields back, so the workbench can show
// a single tab strip beside a sidebar that swaps with the bottom-bar menu — Zed-style, one strip for everything.
struct TabMove: Equatable { var moving: String; var target: String; var seq: Int }

struct TabCloseCmd: Equatable {
    enum Kind { case others, right, left, all }
    var kind: Kind
    var anchor: String
    var seq: Int
}

@MainActor
@Observable final class Workbench {
    // Published by FilesPane (the editor).
    var tabs: [WorkTab] = []
    var active: String?
    var activeFile: WorkspaceFileEntry?   // the active editor tab's file, for the sidebar tree to highlight
    var treeRoots: [WFileTreeNode] = []
    var treeExpanded: Set<String> = []
    var treeDirty: Set<String> = []
    var treeVersion = 0

    // Commands sent down to FilesPane (consumed + cleared by it).
    var openCmd: WorkspaceFileEntry?            // single click in the tree -> preview tab (italic)
    var openPermanentCmd: WorkspaceFileEntry?   // double click in the tree -> permanent tab
    var moveTab: TabMove?                        // drag-reorder a tab within this pane (moving before target)
    var activateCmd: String?
    var closeCmd: String?
    var tabCloseCmd: TabCloseCmd?
    var openGitDiffCmd: WorkspaceFileEntry?   // git panel click -> open a read-only diff tab in the editor
    var promoteCmd: String?   // double-click a preview tab -> make it permanent

    func tabClose(_ kind: TabCloseCmd.Kind, _ anchor: String) {
        tabCloseCmd = TabCloseCmd(kind: kind, anchor: anchor, seq: (tabCloseCmd?.seq ?? 0) + 1)
    }

    var hasTabs: Bool { !tabs.isEmpty }
    var treeVisible = true

    // Zoom (Zed's Maximize): hide the file-tree sidebar + the app's outer sidebar so the editor fills the window.
    // Remember what to restore on zoom out.
    var zoomed = false
    var preZoomTreeVisible = true
    var preZoomSidebarCollapsed = false

    // Tab-history navigation (back/forward arrows), published + driven by FilesPane.
    var canBack = false
    var canForward = false
    var backCmd = 0
    var forwardCmd = 0

    // The tree is owned + loaded here (not published up from FilesPane) so it never depends on FilesPane's publish
    // timing — that made the sidebar tree flaky/empty.
    func loadTree(branch: String, isMain: Bool, path: String) async {
        let raw = await Task.detached(priority: .userInitiated) {
            FileStore.list(branch: branch, isMain: isMain)
        }.value
        let rootName = (path as NSString).lastPathComponent
        let built = await Task.detached(priority: .userInitiated) { () -> [WFileTreeNode] in
            let list = PomJSON.decode([WorkspaceFileEntry].self, from: raw) ?? []
            return WFileTreeBuilder.build(list, rootName: rootName)
        }.value
        treeRoots = built
        treeVersion &+= 1
        if treeExpanded.isEmpty { treeExpanded.insert("") }
    }
}
