import SwiftUI


struct DiffLine: Identifiable, Sendable, Decodable, Equatable {
    enum Kind: String, Sendable, Decodable, Equatable { case context, add, del, hunk }
    let id: Int
    let kind: Kind
    let oldN: Int?
    let newN: Int?
    let text: String
    enum CodingKeys: String, CodingKey { case id, kind, text, oldN = "old_n", newN = "new_n" }
}

struct DiffFile: Identifiable, Sendable, Decodable {
    var path: String
    var oldPath: String?
    var status: String
    var adds: Int = 0
    var dels: Int = 0
    var binary = false
    var lines: [DiffLine] = []
    var headerOldPath: String = ""
    var id: String { path }

    enum CodingKeys: String, CodingKey {
        case path, status, adds, dels, binary, lines
        case oldPath = "old_path", headerOldPath = "header_old_path"
    }

    var isRename: Bool { status == "R" || status == "C" }

    /// Longest common directory prefix of old and new path, so a rename can be
    /// shown as `dir/{old => new}` instead of two near-identical full paths.
    var renameParts: (prefix: String, from: String, to: String)? {
        guard isRename, let old = oldPath, old != path else { return nil }
        let a = old.split(separator: "/").map(String.init)
        let b = path.split(separator: "/").map(String.init)
        var i = 0
        while i < a.count - 1 && i < b.count - 1 && a[i] == b[i] { i += 1 }
        let prefix = i == 0 ? "" : a[0..<i].joined(separator: "/") + "/"
        return (prefix, a[i...].joined(separator: "/"), b[i...].joined(separator: "/"))
    }
}

final class FileTreeNode: Identifiable {
    var id: String
    var name: String
    let file: DiffFile?
    var children: [FileTreeNode] = []
    init(id: String, name: String, file: DiffFile? = nil) { self.id = id; self.name = name; self.file = file }
    var isLeaf: Bool { file != nil }
}

enum FileTreeBuilder {
    static func build(_ files: [DiffFile]) -> [FileTreeNode] {
        let root = FileTreeNode(id: "", name: "")
        for f in files {
            var node = root
            let parts = f.path.split(separator: "/").map(String.init)
            for (i, part) in parts.enumerated() {
                let isLast = i == parts.count - 1
                if let existing = node.children.first(where: { $0.name == part && $0.isLeaf == isLast }) {
                    node = existing
                } else {
                    let childID = node.id.isEmpty ? part : "\(node.id)/\(part)"
                    let child = FileTreeNode(id: childID, name: part, file: isLast ? f : nil)
                    node.children.append(child)
                    node = child
                }
            }
        }
        // Collapse the children, not the root: a chain folded into the root itself
        // (`apps/portal/src`) would be dropped when only root.children is returned.
        for child in root.children { collapse(child) }
        sort(root)
        return root.children
    }

    // Fold single-child chains into one row (`a/b/c`) to save indent. `id` stays
    // the full path — it keys the collapse state.
    private static func collapse(_ node: FileTreeNode) {
        for child in node.children { collapse(child) }
        while node.children.count == 1, let only = node.children.first, !only.isLeaf {
            node.name = node.name.isEmpty ? only.name : "\(node.name)/\(only.name)"
            node.id = only.id
            node.children = only.children
        }
    }

    private static func sort(_ node: FileTreeNode) {
        node.children.sort { a, b in
            if a.isLeaf != b.isLeaf { return !a.isLeaf }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
        for child in node.children { sort(child) }
    }
}

struct DiffFileList: View {
    @EnvironmentObject var theme: ThemeManager
    let files: [DiffFile]
    @Binding var selected: String?
    @State private var collapsed: Set<String> = []
    // Built once per file list: recomputing it every render reallocates the whole
    // node graph.
    @State private var tree: [FileTreeNode] = []

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 1) {
                ForEach(flattened(tree, depth: 0), id: \.node.id) { entry in row(entry.node, depth: entry.depth) }
            }.padding(6)
        }
        .task(id: files.map(\.id).joined(separator: "\n")) {
            tree = FileTreeBuilder.build(files)
        }
    }

    private func flattened(_ nodes: [FileTreeNode], depth: Int) -> [(node: FileTreeNode, depth: Int)] {
        var out: [(node: FileTreeNode, depth: Int)] = []
        for node in nodes {
            out.append((node, depth))
            if !node.isLeaf && !collapsed.contains(node.id) {
                out.append(contentsOf: flattened(node.children, depth: depth + 1))
            }
        }
        return out
    }

    @ViewBuilder private func row(_ node: FileTreeNode, depth: Int) -> some View {
        if let f = node.file {
            TreeRow(depth: depth, indent: indent, isDir: false, expanded: false, name: node.name,
                    leadingSymbol: nil, marker: (f.status, statusColor(f.status)),
                    selected: selected == f.path, selectionColor: Theme.sel, nameColor: Theme.fg,
                    nameWeight: .regular, tooltip: leafTooltip(f, label: node.name)) { selected = f.path }
        } else {
            let isCollapsed = collapsed.contains(node.id)
            TreeRow(depth: depth, indent: indent, isDir: true, expanded: !isCollapsed, name: node.name,
                    leadingSymbol: "folder.fill", marker: nil, selected: false, selectionColor: Theme.sel,
                    nameColor: Theme.fgMuted, nameWeight: .medium, tooltip: node.name) { toggle(node.id) }
        }
    }

    // The row sits under its folder rows, so the label alone is enough — repeating
    // the full repo path is noise. A rename also shows where the file came from.
    private func leafTooltip(_ f: DiffFile, label: String) -> String {
        guard let r = f.renameParts else { return label }
        return "\(r.from)\n→ \(r.to)"
    }

    // Indent tapers past a few levels so deep trees keep room for the filename.
    private func indent(_ depth: Int) -> CGFloat {
        let full = min(depth, 4)
        let extra = max(0, depth - 4)
        return CGFloat(full) * 12 + CGFloat(extra) * 5
    }

    private func toggle(_ id: String) {
        if collapsed.contains(id) { collapsed.remove(id) } else { collapsed.insert(id) }
    }

    private func statusColor(_ s: String) -> Color {
        switch s { case "A": return Theme.ok; case "D": return Theme.danger; case "R": return Theme.accent; default: return Theme.warn }
    }
}

struct DiffFilesView: View {
    @EnvironmentObject var theme: ThemeManager
    @ObservedObject private var codeDisplay = CodeDisplayManager.shared
    let files: [DiffFile]?
    @Binding var selFile: String?
    @Binding var filesTreeVisible: Bool
    @Binding var splitDiff: Bool
    let loadingLabel: String
    let emptyLabel: String

    var body: some View {
        Group {
            if let files {
                if files.isEmpty {
                    EmptyStateView(icon: "doc.text", title: emptyLabel)
                } else {
                    HStack(spacing: 0) {
                        if filesTreeVisible {
                            DiffFileList(files: files, selected: $selFile).frame(width: 260)
                            Divider().overlay(Theme.borderSoft)
                        }
                        VStack(spacing: 0) {
                            diffModeBar(path: selFile)
                            Divider().overlay(Theme.borderSoft)
                            if let f = files.first(where: { $0.path == selFile }) {
                                if f.lines.isEmpty && !f.binary {
                                    renamedPlaceholder(f)
                                } else if f.binary {
                                    centered("Binary file — no textual diff")
                                } else if splitDiff {
                                    SplitDiffRibbon(file: f, isDark: theme.mode.isDark)
                                } else {
                                    CodeDiffView(file: f, isDark: theme.mode.isDark, wrapMode: codeDisplay.wrapMode)
                                }
                            } else {
                                centered("Select a file")
                            }
                        }
                    }
                }
            } else {
                LoadingView(text: loadingLabel)
            }
        }
    }

    private func diffModeBar(path: String?) -> some View {
        let file = files?.first { $0.path == path }
        return HStack(spacing: 4) {
            Button { withAnimation(.easeInOut(duration: 0.14)) { filesTreeVisible.toggle() } } label: {
                Image(systemName: "sidebar.left").font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
            }.buttonStyle(.plain).help(filesTreeVisible ? "Hide file list" : "Show file list")
            if let r = file?.renameParts {
                VStack(alignment: .leading, spacing: 1) {
                    pathLine(r.prefix + r.from, icon: "minus", color: Theme.danger, dim: true)
                    pathLine(r.prefix + r.to, icon: "plus", color: Theme.ok, dim: false)
                }
                .layoutPriority(1)
            } else if let path {
                Text(path).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted).lineLimit(1).truncationMode(.middle)
                    .textSelection(.enabled)
            }
            Spacer()
            if let f = file, f.adds > 0 || f.dels > 0 {
                HStack(spacing: 6) {
                    if f.adds > 0 { Text("+\(f.adds)").foregroundStyle(Theme.ok) }
                    if f.dels > 0 { Text("-\(f.dels)").foregroundStyle(Theme.danger) }
                }
                .font(Theme.mono(10.5)).lineLimit(1).fixedSize()
                .padding(.trailing, 4)
            }
            modeBtn("Unified", "list.bullet", on: !splitDiff) { splitDiff = false }
            modeBtn("Split", "rectangle.split.2x1", on: splitDiff) { splitDiff = true }
        }
        .padding(.horizontal, 10).padding(.vertical, 5)
        .background(Theme.bgSoft)
    }

    private func pathLine(_ path: String, icon: String, color: Color, dim: Bool) -> some View {
        HStack(spacing: 5) {
            Image(systemName: icon).font(.system(size: 8, weight: .bold)).foregroundStyle(color).frame(width: 8)
            Text(path).font(Theme.mono(10.5)).foregroundStyle(dim ? Theme.dim : Theme.fgMuted)
                .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
        }
    }

    private func modeBtn(_ label: String, _ icon: String, on: Bool, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            HStack(spacing: 4) { Image(systemName: icon).font(.system(size: 10)); Text(label).font(.system(size: 11)) }
                .foregroundStyle(on ? Theme.accent : Theme.fgMuted)
                .padding(.horizontal, 8).padding(.vertical, 3)
                .background(on ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 6))
        }.buttonStyle(.plain)
    }

    @ViewBuilder private func renamedPlaceholder(_ f: DiffFile) -> some View {
        if let r = f.renameParts {
            VStack(spacing: 10) {
                Image(systemName: "arrow.triangle.turn.up.right.diamond")
                    .font(.system(size: 22)).foregroundStyle(Theme.dim)
                Text("Renamed — no content changes").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                VStack(spacing: 3) {
                    Text(r.prefix + r.from).foregroundStyle(Theme.dim)
                    Text(r.prefix + r.to).foregroundStyle(Theme.fgMuted)
                }
                .font(Theme.mono(11)).textSelection(.enabled)
                .multilineTextAlignment(.center)
            }
            .padding(24)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            centered("No changes to display")
        }
    }

    private func centered(_ s: String) -> some View {
        Text(s).font(.system(size: 12)).foregroundStyle(Theme.dim)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct SplitRow: Identifiable, Sendable {
    let id: Int
    var hunk: String? = nil
    var leftN: Int? = nil;  var left: String? = nil;  var leftHi: Range<Int>? = nil;  var leftSpans: [SynSpan] = []
    var rightN: Int? = nil; var right: String? = nil; var rightHi: Range<Int>? = nil; var rightSpans: [SynSpan] = []
    var changed = false
}

private func middleDiff(_ a: String, _ b: String) -> (Range<Int>, Range<Int>) {
    let ac = Array(a), bc = Array(b)
    var p = 0
    while p < ac.count && p < bc.count && ac[p] == bc[p] { p += 1 }
    var s = 0
    while s < ac.count - p && s < bc.count - p && ac[ac.count - 1 - s] == bc[bc.count - 1 - s] { s += 1 }
    return (p..<(ac.count - s), p..<(bc.count - s))
}

func splitRows(_ file: DiffFile) -> [SplitRow] {
    var rows: [SplitRow] = []
    var dels: [DiffLine] = [], adds: [DiffLine] = []
    var rid = 0
    func flush() {
        let n = max(dels.count, adds.count)
        for i in 0..<n {
            rid += 1
            let d = i < dels.count ? dels[i] : nil
            let a = i < adds.count ? adds[i] : nil
            var lHi: Range<Int>?, rHi: Range<Int>?
            if let d, let a, d.text != a.text, d.text.count < 400, a.text.count < 400 {
                (lHi, rHi) = middleDiff(d.text, a.text)
            }
            rows.append(SplitRow(id: rid, leftN: d?.oldN, left: d?.text, leftHi: lHi, leftSpans: d.map { Syntax.spans($0.text) } ?? [],
                                 rightN: a?.newN, right: a?.text, rightHi: rHi, rightSpans: a.map { Syntax.spans($0.text) } ?? [], changed: true))
        }
        dels.removeAll(); adds.removeAll()
    }
    for l in file.lines {
        switch l.kind {
        case .hunk:    flush(); rid += 1; rows.append(SplitRow(id: rid, hunk: l.text))
        case .del:     dels.append(l)
        case .add:     adds.append(l)
        case .context:
            flush(); rid += 1
            let sp = Syntax.spans(l.text)
            rows.append(SplitRow(id: rid, leftN: l.oldN, left: l.text, leftSpans: sp, rightN: l.newN, right: l.text, rightSpans: sp))
        }
    }
    flush()
    return rows
}


import AppKit

struct CodeDiffView: NSViewRepresentable {
    let file: DiffFile
    var isDark: Bool
    var wrapMode: CodeWrapMode = activeCodeWrapMode

    func makeCoordinator() -> Coord { Coord() }

    func makeNSView(context: Context) -> NSScrollView {
        let tv = CodeTextView()
        tv.configureReadOnly(inset: NSSize(width: 0, height: 6))
        context.coordinator.textView = tv
        return CodeTextView.makeScroll(tv)
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let ap = NSAppearance(named: isDark ? .darkAqua : .aqua)
        scroll.appearance = ap
        guard let tv = context.coordinator.textView else { return }
        tv.appearance = ap
        tv.configureReadOnly(inset: NSSize(width: 0, height: 6), wraps: wrapMode.wraps)
        scroll.hasHorizontalScroller = !wrapMode.wraps
        // Theme-derived colours are baked into the string at build time, so a theme
        // switch must rebuild — hence isDark in the key.
        let key = "\(file.path):\(isDark):\(wrapMode.rawValue)"
        if context.coordinator.key == key { return }
        context.coordinator.key = key
        tv.apply(CodeTextView.diff(file))
    }

    final class Coord { var textView: CodeTextView?; var key = "" }
}

extension CodeTextView {
    static func diff(_ file: DiffFile) -> CodeModel {
        let mono = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        let para = paragraph(lineHeight: 19)
        let dim = NSColor.tertiaryLabelColor, code = NSColor.labelColor
        let addC = NSColor.systemGreen, delC = NSColor.systemRed, hunkC = NSColor.systemPurple
        let addBg = opaque(.systemGreen, 0.16), delBg = opaque(.systemRed, 0.16), hunkBg = opaque(.systemPurple, 0.10)
        let out = NSMutableAttributedString()
        var starts: [Int] = [], bg: [NSColor?] = [], cells: [GutterCell] = []
        func append(_ s: String, _ color: NSColor) {
            out.append(NSAttributedString(string: s, attributes: CodeTextView.rowAttributes(font: mono, color: color, lineHeight: para.maximumLineHeight)))
        }
        for l in file.lines {
            starts.append(out.length)
            switch l.kind {
            case .hunk:
                bg.append(hunkBg)
                cells.append(GutterCell())
                append(l.text + "\n", hunkC)
            default:
                bg.append(l.kind == .add ? addBg : l.kind == .del ? delBg : nil)
                cells.append(GutterCell(columns: [l.oldN.map(String.init) ?? "", l.newN.map(String.init) ?? ""],
                                        sign: l.kind == .add ? "+" : l.kind == .del ? "-" : "",
                                        signColor: l.kind == .add ? addC : l.kind == .del ? delC : dim))
                out.append(attributedLine(l.text + "\n", spans: Syntax.spans(l.text), font: mono, base: code, paragraph: para))
            }
        }
        let maxN = cells.compactMap { $0.columns.compactMap(Int.init).max() }.max() ?? 0
        return CodeModel(string: out, starts: starts, lineBg: bg, gutters: cells,
                         gutterWidth: CodeTextView.Gutter.width(columns: 2, maxLine: maxN, sign: true))
    }
}
