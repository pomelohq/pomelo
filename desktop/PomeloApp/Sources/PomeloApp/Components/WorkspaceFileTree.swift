import SwiftUI
import AppKit

struct WorkspaceFileEntry: Identifiable, Decodable {
    var repo: String
    var path: String
    var isDir: Bool
    var size: Int64
    var id: String { "\(repo)/\(path)" }

    enum CodingKeys: String, CodingKey { case repo, path, isDir = "is_dir", size }

    init(repo: String, path: String, isDir: Bool, size: Int64 = 0) {
        self.repo = repo; self.path = path; self.isDir = isDir; self.size = size
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        repo = try c.decode(String.self, forKey: .repo)
        path = try c.decode(String.self, forKey: .path)
        isDir = try c.decodeIfPresent(Bool.self, forKey: .isDir) ?? false
        size = try c.decodeIfPresent(Int64.self, forKey: .size) ?? 0
    }
}

final class WFileTreeNode: Identifiable {
    var id: String
    var name: String
    private(set) var entry: WorkspaceFileEntry?
    var children: [WFileTreeNode] = []
    fileprivate var index: [String: WFileTreeNode] = [:]
    let isRoot: Bool
    init(id: String, name: String, entry: WorkspaceFileEntry? = nil, isRoot: Bool = false) {
        self.id = id; self.name = name; self.entry = entry; self.isRoot = isRoot
    }
    var isLeaf: Bool { entry != nil && !(entry?.isDir ?? false) }

    fileprivate func attach(_ e: WorkspaceFileEntry) { entry = e }
}

enum WFileTreeBuilder {
    // Everything hangs off one workspace-root node (like Zed), so the tree shows the
    // folder you actually opened rather than a bare list of repos.
    static func build(_ entries: [WorkspaceFileEntry], rootName: String) -> [WFileTreeNode] {
        var repoRoots: [String: WFileTreeNode] = [:]
        var looseFiles: [WFileTreeNode] = []
        for e in entries.sorted(by: { $0.path < $1.path }) {
            guard !e.repo.isEmpty else {
                looseFiles.append(WFileTreeNode(id: e.path, name: e.path, entry: e))
                continue
            }
            let root = repoRoots[e.repo] ?? {
                let r = WFileTreeNode(id: e.repo, name: e.repo)
                repoRoots[e.repo] = r
                return r
            }()
            var node = root
            for part in e.path.split(separator: "/").map(String.init) {
                if let existing = node.index[part] {
                    node = existing
                } else {
                    let child = WFileTreeNode(id: "\(node.id)/\(part)", name: part)
                    node.index[part] = child
                    node.children.append(child)
                    node = child
                }
            }
            node.attach(e)
        }
        let ordered = repoRoots.keys.sorted().map { repoRoots[$0]! }
        for r in ordered { sort(r) }
        looseFiles.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        let root = WFileTreeNode(id: "", name: rootName, isRoot: true)
        root.children = ordered + looseFiles
        return [root]
    }

    private static func sort(_ node: WFileTreeNode) {
        node.children.sort { a, b in
            if a.isLeaf != b.isLeaf { return !a.isLeaf }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
        for child in node.children { sort(child) }
    }

    // All directory node ids in the tree — used for Expand All.
    static func dirIDs(_ nodes: [WFileTreeNode]) -> [String] {
        var out: [String] = []
        for n in nodes where !n.isLeaf {
            out.append(n.id)
            out.append(contentsOf: dirIDs(n.children))
        }
        return out
    }
}

struct WorkspaceFileTreeList: View {
    let roots: [WFileTreeNode]
    let workspacePath: String
    @Binding var selected: WorkspaceFileEntry?
    @Binding var expanded: Set<String>

    @State private var ctxNodeID: String?
    @State private var renamingID: String?
    @State private var renameText = ""

    var body: some View {
        let flat = flattened(roots, depth: 0)
        FileTreeTable(rows: flat, contentWidth: contentWidth(flat), signature: signature(flat)) { node, depth in
            AnyView(row(node, depth: depth))
        }
        .background(Theme.bg)
    }

    // Approximate widest row so the table can scroll horizontally to reveal long names.
    private func contentWidth(_ flat: [(node: WFileTreeNode, depth: Int)]) -> CGFloat {
        var maxW: CGFloat = 0
        for (n, d) in flat {
            let w = CGFloat(d) * 13 + 46 + CGFloat(n.name.count) * 7
            if w > maxW { maxW = w }
        }
        return maxW
    }

    // Reload the table only when structure / selection / edit state changes — not on
    // every rename keystroke (that would rebuild the editing cell and drop focus).
    private func signature(_ flat: [(node: WFileTreeNode, depth: Int)]) -> Int {
        var h = Hasher()
        h.combine(flat.count)
        h.combine(selected?.id)
        h.combine(renamingID)
        h.combine(ctxNodeID)
        h.combine(expanded)
        return h.finalize()
    }

    private func flattened(_ nodes: [WFileTreeNode], depth: Int) -> [(node: WFileTreeNode, depth: Int)] {
        var out: [(node: WFileTreeNode, depth: Int)] = []
        for node in nodes {
            out.append((node, depth))
            if !node.isLeaf && expanded.contains(node.id) {
                out.append(contentsOf: flattened(node.children, depth: depth + 1))
            }
        }
        return out
    }

    @ViewBuilder private func row(_ node: WFileTreeNode, depth: Int) -> some View {
        let isDir = !node.isLeaf
        let matName: String? = isDir ? "mi-" + MaterialIcon.folder(node.name) : MaterialIcon.file(node.name).map { "mi-" + $0 }
        TreeRow(depth: depth, indent: { CGFloat($0) * 13 }, isDir: isDir, expanded: expanded.contains(node.id), name: node.name,
                leadingSymbol: "doc",
                marker: nil,
                selected: node.isLeaf && selected?.id == node.entry?.id,
                selectionColor: Theme.fg.opacity(0.13),
                nameColor: node.isRoot ? Theme.fg : (node.isLeaf ? Theme.fg : Theme.fgMuted),
                nameWeight: node.isLeaf ? .regular : (node.isRoot ? .semibold : .medium),
                tooltip: nil,
                editing: renamingID == node.id, editText: $renameText,
                onCommitEdit: { commitRename(node) }, onCancelEdit: { renamingID = nil },
                truncate: false,
                showChevron: false, guides: depth, hoverHighlight: true,
                hoverColor: Theme.fg.opacity(0.06), cornerRadius: 0,
                selectedBorder: nil, iconColor: nil, leadingImageName: matName, fillWidth: true) {
            if node.isLeaf, let e = node.entry { selected = e } else { toggle(node.id) }
        }
        .overlay(RightClickArea { ctxNodeID = node.id })
        .popover(isPresented: Binding(get: { ctxNodeID == node.id }, set: { if !$0 { ctxNodeID = nil } }),
                 arrowEdge: .leading) {
            menuContent(for: node, isDir: isDir)
        }
    }

    // MARK: - Custom context menu (themed; not the native NSMenu)

    @ViewBuilder private func menuContent(for node: WFileTreeNode, isDir: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if isDir {
                MenuItem("New File", "doc.badge.plus") { newFile(in: node) }
                MenuItem("New Folder", "folder.badge.plus") { newFolder(in: node) }
                menuDivider
            }
            MenuItem(isDir ? "Open in Finder" : "Reveal in Finder", "magnifyingglass") { reveal(node, isDir: isDir) }
            if isDir { MenuItem("Open in Terminal", "terminal") { openInTerminal(node) } }
            menuDivider
            MenuItem("Copy Path", "doc.on.clipboard") { copyToClipboard(absolutePath(node)) }
            if !node.isRoot { MenuItem("Copy Relative Path") { copyToClipboard(node.id) } }
            if !node.isRoot {
                menuDivider
                MenuItem("Rename", "pencil") { beginRename(node) }
                MenuItem("Duplicate", "plus.square.on.square") { duplicate(node) }
                if node.id.contains("/") { MenuItem("Add to .gitignore") { addToGitignore(node) } }
                menuDivider
                MenuItem("Move to Trash", "trash", destructive: true) { trash(node) }
            }
            if isDir {
                menuDivider
                MenuItem("Expand All") { expanded.formUnion(WFileTreeBuilder.dirIDs(roots)) }
                MenuItem("Collapse All") { expanded = [""] }
            }
        }
        .frame(width: 214)
        .padding(.vertical, 5)
        .background(Theme.bgSoft)
    }

    private var menuDivider: some View {
        Rectangle().fill(Theme.borderSoft).frame(height: 1).padding(.vertical, 4).padding(.horizontal, 8)
    }

    // Closes the menu, then runs the action (nothing here mutates while presented).
    private func MenuItem(_ label: String, _ symbol: String? = nil, destructive: Bool = false,
                          _ action: @escaping () -> Void) -> some View {
        MenuItemView(label: label, symbol: symbol, destructive: destructive) {
            ctxNodeID = nil
            action()
        }
    }

    // MARK: - Paths

    private func absolutePath(_ node: WFileTreeNode) -> String {
        (workspacePath as NSString).appendingPathComponent(node.id)
    }

    private func containerDir(_ node: WFileTreeNode) -> String {
        node.isLeaf ? (absolutePath(node) as NSString).deletingLastPathComponent : absolutePath(node)
    }

    // MARK: - Actions

    private func copyToClipboard(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    private func reveal(_ node: WFileTreeNode, isDir: Bool) {
        let url = URL(fileURLWithPath: absolutePath(node))
        if isDir { NSWorkspace.shared.open(url) }
        else { NSWorkspace.shared.activateFileViewerSelecting([url]) }
    }

    private func openInTerminal(_ node: WFileTreeNode) {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        p.arguments = ["-a", "Terminal", absolutePath(node)]
        try? p.run()
    }

    private func newFile(in node: WFileTreeNode) {
        let dir = containerDir(node)
        let name = uniqueName("Untitled", in: dir)
        FileManager.default.createFile(atPath: (dir as NSString).appendingPathComponent(name), contents: Data())
        expanded.insert(node.id)
        startInlineRename(id: node.id.isEmpty ? name : node.id + "/" + name, text: name)
    }

    private func newFolder(in node: WFileTreeNode) {
        let dir = containerDir(node)
        let name = uniqueName("Untitled Folder", in: dir)
        try? FileManager.default.createDirectory(atPath: (dir as NSString).appendingPathComponent(name),
                                                 withIntermediateDirectories: true)
        expanded.insert(node.id)
        startInlineRename(id: node.id.isEmpty ? name : node.id + "/" + name, text: name)
    }

    private func beginRename(_ node: WFileTreeNode) { startInlineRename(id: node.id, text: node.name) }

    // The new/renamed node may not be in the tree yet (FSEvents is async); once its
    // row appears it renders the inline field because renamingID matches.
    private func startInlineRename(id: String, text: String) {
        renameText = text
        renamingID = id
    }

    private func commitRename(_ node: WFileTreeNode) {
        let newName = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
        renamingID = nil
        guard !newName.isEmpty, newName != node.name, !newName.contains("/") else { return }
        let src = absolutePath(node)
        let dst = ((src as NSString).deletingLastPathComponent as NSString).appendingPathComponent(newName)
        do {
            try FileManager.default.moveItem(atPath: src, toPath: dst)
            if selected?.id == node.entry?.id, let e = node.entry {
                selected = WorkspaceFileEntry(repo: e.repo, path: renameRelPath(e.path, to: newName), isDir: e.isDir)
            }
        } catch { warn(error.localizedDescription) }
    }

    private func renameRelPath(_ old: String, to newName: String) -> String {
        let parent = (old as NSString).deletingLastPathComponent
        return parent.isEmpty ? newName : parent + "/" + newName
    }

    private func duplicate(_ node: WFileTreeNode) {
        let src = absolutePath(node)
        let ext = (node.name as NSString).pathExtension
        let base = (node.name as NSString).deletingPathExtension
        let dir = (src as NSString).deletingLastPathComponent
        let name = uniqueName(base + " copy", in: dir, ext: ext)
        try? FileManager.default.copyItem(atPath: src, toPath: (dir as NSString).appendingPathComponent(name))
    }

    private func trash(_ node: WFileTreeNode) {
        try? FileManager.default.trashItem(at: URL(fileURLWithPath: absolutePath(node)), resultingItemURL: nil)
        if selected?.id == node.entry?.id { selected = nil }
    }

    private func addToGitignore(_ node: WFileTreeNode) {
        let comps = node.id.split(separator: "/").map(String.init)
        guard let repo = comps.first else { return }
        let rel = comps.dropFirst().joined(separator: "/")
        guard !rel.isEmpty else { return }
        let gitignore = (workspacePath as NSString).appendingPathComponent(repo) + "/.gitignore"
        var body = (try? String(contentsOfFile: gitignore, encoding: .utf8)) ?? ""
        let line = "/" + rel
        guard !body.split(separator: "\n").contains(where: { $0.trimmingCharacters(in: .whitespaces) == line }) else { return }
        if !body.isEmpty && !body.hasSuffix("\n") { body += "\n" }
        body += line + "\n"
        try? body.write(toFile: gitignore, atomically: true, encoding: .utf8)
    }

    private func uniqueName(_ base: String, in dir: String, ext: String = "") -> String {
        let suffix = ext.isEmpty ? "" : "." + ext
        var name = base + suffix
        var i = 2
        while FileManager.default.fileExists(atPath: (dir as NSString).appendingPathComponent(name)) {
            name = base + " \(i)" + suffix; i += 1
        }
        return name
    }

    private func toggle(_ id: String) {
        if expanded.contains(id) { expanded.remove(id) } else { expanded.insert(id) }
    }

    private func warn(_ message: String) {
        let alert = NSAlert()
        alert.messageText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}

// Maps a file/folder name to a bundled Material Icon Theme asset (mi-<name>), the
// VS Code icon set (material-extensions/vscode-material-icon-theme, MIT). Returns nil
// for unmapped files so the tree falls back to a generic doc glyph.
enum MaterialIcon {
    static func file(_ name: String) -> String? {
        let lower = name.lowercased()
        switch lower {
        case "dockerfile", ".dockerignore", "docker-compose.yml", "docker-compose.yaml": return "docker"
        case "makefile", "rakefile", "gnumakefile": return "makefile"
        case "gemfile", "gemfile.lock", ".gemspec": return "ruby"
        case "package.json": return "nodejs"
        case "package-lock.json": return "npm"
        case "readme", "readme.md": return "readme"
        case "license", "licence", "license.md", "copying": return "license"
        case ".gitignore", ".gitattributes", ".gitmodules", ".gitkeep": return "git"
        case "tsconfig.json", "tsconfig.build.json": return "typescript"
        case "nginx.conf": return "nginx"
        case "schema.prisma": return "prisma"
        default: break
        }
        if lower.hasPrefix(".env") { return "tune" }
        if lower.hasPrefix(".git") { return "git" }
        if lower.hasPrefix(".eslint") { return "eslint" }
        if lower.hasPrefix(".prettier") { return "prettier" }
        if lower.contains("tailwind") { return "tailwindcss" }

        switch (lower as NSString).pathExtension {
        case "js", "cjs", "mjs": return "javascript"
        case "jsx": return "react"
        case "ts": return "typescript"
        case "tsx": return "react_ts"
        case "json", "json5", "jsonc": return "json"
        case "yml", "yaml": return "yaml"
        case "md", "markdown", "mdx": return "markdown"
        case "html", "htm": return "html"
        case "css": return "css"
        case "scss", "sass": return "sass"
        case "less": return "less"
        case "rb", "erb", "gemspec", "ru": return "ruby"
        case "py", "pyi": return "python"
        case "go": return "go"
        case "rs": return "rust"
        case "java": return "java"
        case "kt", "kts": return "kotlin"
        case "php": return "php"
        case "c": return "c"
        case "h", "hpp": return "h"
        case "cpp", "cc", "cxx": return "cpp"
        case "cs": return "csharp"
        case "swift": return "swift"
        case "vue": return "vue"
        case "svelte": return "svelte"
        case "sh", "bash", "zsh", "fish": return "console"
        case "sql": return "database"
        case "csv", "tsv": return "table"
        case "xml", "plist": return "xml"
        case "svg": return "svg"
        case "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "icns": return "image"
        case "pdf": return "pdf"
        case "zip", "tar", "gz", "tgz", "rar": return "zip"
        case "lock": return "lock"
        case "log": return "log"
        case "txt", "text": return "document"
        case "toml", "ini", "conf", "cfg", "properties": return "settings"
        case "graphql", "gql": return "graphql"
        case "prisma": return "prisma"
        case "tf", "tfvars": return "terraform"
        default: return nil
        }
    }

    static func folder(_ name: String) -> String {
        switch name.lowercased() {
        case "src", "lib": return "folder-src"
        case "app": return "folder-app"
        case "dist", "build", "out": return "folder-dist"
        case "public", "static", "assets": return "folder-public"
        case "test", "tests", "spec", "__tests__": return "folder-test"
        case "docs", "doc", "documentation": return "folder-docs"
        case "node_modules": return "folder-node"
        case ".github": return "folder-github"
        case ".git": return "folder-git"
        case "components", "component": return "folder-components"
        case "config", ".config": return "folder-config"
        default: return "folder-base"
        }
    }
}

// A themed context-menu row with hover highlight.
private struct MenuItemView: View {
    let label: String
    var symbol: String? = nil
    var destructive = false
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if let symbol {
                    Image(systemName: symbol).font(.system(size: 11)).frame(width: 15)
                        .foregroundStyle(destructive ? Theme.danger : Theme.fgMuted)
                }
                Text(label).font(.system(size: 12)).foregroundStyle(destructive ? Theme.danger : Theme.fg)
                Spacer(minLength: 12)
            }
            .padding(.horizontal, 10).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(hover ? Theme.sel : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

// Catches right-clicks to trigger the custom menu while letting left-clicks fall
// through to the SwiftUI row below (hitTest only claims the event for right buttons).
private struct RightClickArea: NSViewRepresentable {
    let onRightClick: () -> Void
    func makeNSView(context: Context) -> NSView { V(onRightClick: onRightClick) }
    func updateNSView(_ nsView: NSView, context: Context) {}

    final class V: NSView {
        let onRightClick: () -> Void
        init(onRightClick: @escaping () -> Void) { self.onRightClick = onRightClick; super.init(frame: .zero) }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
        override func hitTest(_ point: NSPoint) -> NSView? {
            switch NSApp.currentEvent?.type {
            case .rightMouseDown, .rightMouseUp, .rightMouseDragged: return self
            default: return nil
            }
        }
        override func rightMouseDown(with event: NSEvent) { onRightClick() }
    }
}
