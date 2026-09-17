import SwiftUI

// A single thing a pane can host, ported from Zed's `Item` trait: it carries its own tab metadata and renders its own
// content. Concrete items conform (terminal, diff, and — as their state migrates out of FilesPane — file/search), so a
// pane can render `[any WorkspaceItem]` uniformly and items can move between panes/docks by identity.
@MainActor
protocol WorkspaceItem {
    var itemID: String { get }
    var tabLabel: String { get }
    var tabSystemIcon: String { get }
    var tabMaterialIcon: String? { get }
    var isPreview: Bool { get }
    var isDirty: Bool { get }
    func content() -> AnyView
}

extension WorkspaceItem {
    var tabMaterialIcon: String? { nil }
    var isPreview: Bool { false }
    var isDirty: Bool { false }
}

// A self-contained terminal item: its ptyhost holder outlives the view, so moving it between panes just re-attaches.
struct TerminalItem: WorkspaceItem {
    let holder: String
    let title: String
    let wsKey: String
    let startDir: String
    let themeMode: ThemeMode
    let onClosed: () -> Void

    var itemID: String { "term:" + holder }
    var tabLabel: String { title }
    var tabSystemIcon: String { "terminal" }

    func content() -> AnyView {
        AnyView(MetalTerminalPane(holderName: holder, wsKey: wsKey, startDir: startDir,
                                  themeMode: themeMode, onClosed: onClosed).id(holder))
    }
}

// A self-contained read-only diff item for one file's uncommitted changes.
struct DiffItem: WorkspaceItem {
    let workspace: Workspace
    let entry: WorkspaceFileEntry

    var itemID: String { "gitdiff:\(entry.repo)/\(entry.path)" }
    var tabLabel: String { (entry.path as NSString).lastPathComponent + " (diff)" }
    var tabSystemIcon: String { "plusminus" }

    func content() -> AnyView { AnyView(GitDiffTab(workspace: workspace, entry: entry)) }
}
