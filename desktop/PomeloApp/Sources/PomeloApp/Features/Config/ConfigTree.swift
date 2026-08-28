import SwiftUI

// A node in the config-file tree: pom.yml at the root plus the pom.d/ fragments
// nested by their relative path. Pure model so the build logic is unit-testable.
struct ConfigNode: Identifiable, Hashable {
    let id: String        // file path, or "dir:<rel>" for a folder
    let name: String      // last path component (display)
    let path: String      // file path; empty for a folder
    let isDir: Bool
    var children: [ConfigNode]?
}

enum ConfigTree {
    struct Entry { let rel: String; let path: String }

    // rel is the display path relative to the project dir ("pom.yml",
    // "pom.d/services.yml", "pom.d/backend/api.yml"); path is the absolute file.
    static func build(_ entries: [Entry]) -> [ConfigNode] {
        final class Dir { var files: [(name: String, path: String)] = []; var subs: [String: Dir] = [:] }
        let root = Dir()
        for e in entries {
            let parts = e.rel.split(separator: "/").map(String.init)
            guard let leaf = parts.last else { continue }
            var cur = root
            for p in parts.dropLast() {
                if cur.subs[p] == nil { cur.subs[p] = Dir() }
                cur = cur.subs[p]!
            }
            cur.files.append((leaf, e.path))
        }
        func convert(_ d: Dir, prefix: String) -> [ConfigNode] {
            var out = d.files.sorted { $0.name < $1.name }.map {
                ConfigNode(id: $0.path, name: $0.name, path: $0.path, isDir: false, children: nil)
            }
            for name in d.subs.keys.sorted() {
                let rel = prefix.isEmpty ? name : prefix + "/" + name
                out.append(ConfigNode(id: "dir:" + rel, name: name, path: "",
                                      isDir: true, children: convert(d.subs[name]!, prefix: rel)))
            }
            return out
        }
        return convert(root, prefix: "")
    }
}

// Recursive plain-view row for the config tree. Deliberately not List/OutlineGroup
// (those recurse through AppKit constraint updates inside a fixed-width sheet and
// crash on macOS); a ScrollView of these rows is predictable.
struct ConfigNodeRow: View {
    let node: ConfigNode
    let depth: Int
    let selected: String
    let onSelect: (String) -> Void
    @State private var expanded = true

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            TreeRow(depth: depth, indent: { CGFloat(10 + $0 * 14) }, isDir: node.isDir,
                    expanded: expanded, name: node.name,
                    leadingSymbol: node.isDir ? nil : "doc.text", marker: nil,
                    selected: isSelected, selectionColor: Theme.accentSoft,
                    nameColor: isSelected ? Theme.accent : Theme.fg,
                    nameWeight: .regular, tooltip: node.name) {
                if node.isDir { expanded.toggle() } else { onSelect(node.path) }
            }
            if node.isDir, expanded, let kids = node.children {
                ForEach(kids) { ConfigNodeRow(node: $0, depth: depth + 1, selected: selected, onSelect: onSelect) }
            }
        }
    }

    private var isSelected: Bool { !node.isDir && node.path == selected }
}
