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

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 1) {
                ForEach(flattened(roots, depth: 0), id: \.node.id) { entry in row(entry.node, depth: entry.depth) }
            }.padding(6)
        }
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
        if node.isLeaf, let e = node.entry {
            TreeRow(depth: depth, isDir: false, expanded: false, name: node.name,
                    leadingSymbol: "doc", marker: nil,
                    selected: selected?.id == e.id, selectionColor: Theme.sel, nameColor: Theme.fg,
                    nameWeight: .regular, tooltip: node.name) { selected = e }
                .contextMenu { menu(for: node, isDir: false) }
        } else {
            TreeRow(depth: depth, isDir: true, expanded: expanded.contains(node.id), name: node.name,
                    leadingSymbol: node.isRoot ? "folder.badge.gearshape" : "folder.fill", marker: nil,
                    selected: false, selectionColor: Theme.sel,
                    nameColor: node.isRoot ? Theme.fg : Theme.fgMuted, nameWeight: node.isRoot ? .semibold : .medium,
                    tooltip: node.name) { toggle(node.id) }
                .contextMenu { menu(for: node, isDir: true) }
        }
    }

    @ViewBuilder private func menu(for node: WFileTreeNode, isDir: Bool) -> some View {
        if isDir {
            Button("New File...") { newFile(in: node) }
            Button("New Folder...") { newFolder(in: node) }
            Divider()
        }
        Button(isDir ? "Open Folder in Finder" : "Reveal in Finder") { reveal(node, isDir: isDir) }
        if isDir { Button("Open in Terminal") { openInTerminal(node) } }
        Divider()
        Button("Copy Path") { copyToClipboard(absolutePath(node)) }
        if !node.isRoot { Button("Copy Relative Path") { copyToClipboard(node.id) } }
        if !node.isRoot {
            Divider()
            Button("Rename...") { rename(node) }
            Button("Duplicate") { duplicate(node) }
            if node.id.contains("/") { Button("Add to .gitignore") { addToGitignore(node) } }
            Divider()
            Button("Move to Trash") { trash(node) }
        }
        if isDir {
            Divider()
            Button("Expand All") { expanded.formUnion(WFileTreeBuilder.dirIDs(roots)) }
            Button("Collapse All") { expanded = [""] }
        }
    }

    // MARK: - Paths

    private func absolutePath(_ node: WFileTreeNode) -> String {
        (workspacePath as NSString).appendingPathComponent(node.id)
    }

    // The directory a create/paste acts on: the node itself if a folder, else its parent.
    private func containerDir(_ node: WFileTreeNode) -> String {
        node.isLeaf ? (absolutePath(node) as NSString).deletingLastPathComponent : absolutePath(node)
    }

    // repo + path-within-repo for a node, so a created file can be selected/opened.
    private func repoAndRel(forChildIn node: WFileTreeNode, name: String) -> (repo: String, path: String) {
        if node.isRoot { return ("", name) }
        let comps = node.id.split(separator: "/").map(String.init)
        let repo = comps.first ?? ""
        let rel = comps.dropFirst().joined(separator: "/")
        return (repo, rel.isEmpty ? name : rel + "/" + name)
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
        guard let name = prompt("New File", placeholder: "file name") else { return }
        let dst = (containerDir(node) as NSString).appendingPathComponent(name)
        guard !FileManager.default.fileExists(atPath: dst) else { warn("A file named \"\(name)\" already exists."); return }
        FileManager.default.createFile(atPath: dst, contents: Data())
        let rr = repoAndRel(forChildIn: node, name: name)
        selected = WorkspaceFileEntry(repo: rr.repo, path: rr.path, isDir: false)
    }

    private func newFolder(in node: WFileTreeNode) {
        guard let name = prompt("New Folder", placeholder: "folder name") else { return }
        let dst = (containerDir(node) as NSString).appendingPathComponent(name)
        try? FileManager.default.createDirectory(atPath: dst, withIntermediateDirectories: true)
        expanded.insert(node.id)
    }

    private func rename(_ node: WFileTreeNode) {
        guard let name = prompt("Rename", placeholder: "new name", initial: node.name), name != node.name else { return }
        let src = absolutePath(node)
        let dst = ((src as NSString).deletingLastPathComponent as NSString).appendingPathComponent(name)
        do { try FileManager.default.moveItem(atPath: src, toPath: dst) }
        catch { warn(error.localizedDescription) }
    }

    private func duplicate(_ node: WFileTreeNode) {
        let src = absolutePath(node)
        let ext = (node.name as NSString).pathExtension
        let base = (node.name as NSString).deletingPathExtension
        let dir = (src as NSString).deletingLastPathComponent
        var candidate = base + " copy" + (ext.isEmpty ? "" : "." + ext)
        var i = 2
        while FileManager.default.fileExists(atPath: (dir as NSString).appendingPathComponent(candidate)) {
            candidate = base + " copy \(i)" + (ext.isEmpty ? "" : "." + ext); i += 1
        }
        try? FileManager.default.copyItem(atPath: src, toPath: (dir as NSString).appendingPathComponent(candidate))
    }

    private func trash(_ node: WFileTreeNode) {
        let url = URL(fileURLWithPath: absolutePath(node))
        try? FileManager.default.trashItem(at: url, resultingItemURL: nil)
        if selected?.id == node.entry?.id { selected = nil }
    }

    private func addToGitignore(_ node: WFileTreeNode) {
        let comps = node.id.split(separator: "/").map(String.init)
        guard let repo = comps.first else { return }
        let rel = comps.dropFirst().joined(separator: "/")
        guard !rel.isEmpty else { return }
        let gitignore = (workspacePath as NSString)
            .appendingPathComponent(repo) + "/.gitignore"
        var body = (try? String(contentsOfFile: gitignore, encoding: .utf8)) ?? ""
        let line = "/" + rel
        guard !body.split(separator: "\n").contains(where: { $0.trimmingCharacters(in: .whitespaces) == line }) else { return }
        if !body.isEmpty && !body.hasSuffix("\n") { body += "\n" }
        body += line + "\n"
        try? body.write(toFile: gitignore, atomically: true, encoding: .utf8)
    }

    private func toggle(_ id: String) {
        if expanded.contains(id) { expanded.remove(id) } else { expanded.insert(id) }
    }

    // MARK: - Prompts (AppKit modal — simplest reliable text input on macOS)

    private func prompt(_ title: String, placeholder: String, initial: String = "") -> String? {
        let alert = NSAlert()
        alert.messageText = title
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
        field.placeholderString = placeholder
        field.stringValue = initial
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return nil }
        let v = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        return v.isEmpty ? nil : v
    }

    private func warn(_ message: String) {
        let alert = NSAlert()
        alert.messageText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}
