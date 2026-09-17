import SwiftUI
import AppKit
import CodeEditSourceEditor

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

// A file-editor item. Its text/undo/cursor still live in FilesPane's per-tab storage (passed as bindings) — full
// self-ownership is the remaining migration — but rendering now flows through the item like every other kind.
struct FileItem: WorkspaceItem {
    let entry: WorkspaceFileEntry
    let text: Binding<String>
    let state: Binding<SourceEditorState>
    let mode: ThemeMode
    let fontSize: CGFloat
    let changedLines: [Int: Int]
    let blameLines: [Int: String]
    let focusToken: Int
    let previewTab: Bool
    let dirtyTab: Bool
    let onRightClick: (NSPoint, NSView) -> Void

    var itemID: String { entry.id }
    var tabLabel: String { (entry.path as NSString).lastPathComponent }
    var tabSystemIcon: String { "doc.text" }
    var tabMaterialIcon: String? { MaterialIcon.file(entry.path).map { "mi-" + $0 } }
    var isPreview: Bool { previewTab }
    var isDirty: Bool { dirtyTab }

    func content() -> AnyView {
        AnyView(FileEditor(text: text, path: entry.path, mode: mode, editable: true, fontSize: fontSize,
                           changedLines: changedLines, blameLines: blameLines, onRightClick: onRightClick,
                           focusToken: focusToken, state: state))
    }
}

// The project-search item (Cmd+Shift+F). Its result list is owned by FindInFiles; opening a result routes back through
// the pane's file-open via `onChoose`.
struct SearchItem: WorkspaceItem {
    let branch: String
    let isMain: Bool
    let mode: ThemeMode
    let workspacePath: String
    let query: Binding<String>
    let onChoose: (WorkspaceFileEntry, Int) -> Void
    let onClose: () -> Void

    var itemID: String { "search" }
    var tabLabel: String { "Search" }
    var tabSystemIcon: String { "magnifyingglass" }

    func content() -> AnyView {
        AnyView(FindInFiles(branch: branch, isMain: isMain, mode: mode, workspacePath: workspacePath,
                            query: query, onChoose: onChoose, onClose: onClose))
    }
}
