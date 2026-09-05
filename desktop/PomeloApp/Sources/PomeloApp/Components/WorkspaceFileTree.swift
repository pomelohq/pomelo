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
    init(id: String, name: String, entry: WorkspaceFileEntry? = nil) { self.id = id; self.name = name; self.entry = entry }
    var isLeaf: Bool { entry != nil && !(entry?.isDir ?? false) }

    fileprivate func attach(_ e: WorkspaceFileEntry) { entry = e }
}

enum WFileTreeBuilder {
    static func build(_ entries: [WorkspaceFileEntry]) -> [WFileTreeNode] {
        var roots: [String: WFileTreeNode] = [:]
        for e in entries.sorted(by: { $0.path < $1.path }) {
            let root = roots[e.repo] ?? {
                let r = WFileTreeNode(id: e.repo, name: e.repo)
                roots[e.repo] = r
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
        let ordered = roots.keys.sorted().map { roots[$0]! }
        for root in ordered { sort(root) }
        return ordered
    }

    private static func sort(_ node: WFileTreeNode) {
        node.children.sort { a, b in
            if a.isLeaf != b.isLeaf { return !a.isLeaf }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
        for child in node.children { sort(child) }
    }
}

struct WorkspaceFileTreeList: View {
    let roots: [WFileTreeNode]
    let workspacePath: String
    @Binding var selected: WorkspaceFileEntry?
    @State private var expanded: Set<String> = []

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
                    leadingSymbol: "folder.fill", marker: nil, selected: false, selectionColor: Theme.sel,
                    nameColor: Theme.fgMuted, nameWeight: .medium, tooltip: node.name) { toggle(node.id) }
                .contextMenu { menu(for: node, isDir: true) }
        }
    }

    @ViewBuilder private func menu(for node: WFileTreeNode, isDir: Bool) -> some View {
        Button(isDir ? "Open Folder in Finder" : "Reveal in Finder") { reveal(node, isDir: isDir) }
        Button("Copy Path") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(absolutePath(node), forType: .string)
        }
    }

    private func absolutePath(_ node: WFileTreeNode) -> String {
        (workspacePath as NSString).appendingPathComponent(node.id)
    }

    private func reveal(_ node: WFileTreeNode, isDir: Bool) {
        let path = absolutePath(node)
        if isDir {
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        } else {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
        }
    }

    private func toggle(_ id: String) {
        if expanded.contains(id) { expanded.remove(id) } else { expanded.insert(id) }
    }
}
