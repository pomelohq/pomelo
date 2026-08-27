import SwiftUI


struct DiffLine: Identifiable, Sendable, Decodable {
    enum Kind: String, Sendable, Decodable { case context, add, del, hunk }
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
            Button { selected = f.path } label: {
                HStack(spacing: 7) {
                    Text(f.status).font(Theme.mono(9.5, .bold)).foregroundStyle(statusColor(f.status))
                        .frame(width: 12)
                    Text(node.name).font(.system(size: 11.5)).foregroundStyle(Theme.fg)
                        .lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 4)
                }
                .padding(.leading, indent(depth)).padding(.horizontal, 8).padding(.vertical, 4)
                .background(selected == f.path ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 6))
                .contentShape(Rectangle())
                .tooltip(leafTooltip(f, label: node.name))
            }.buttonStyle(.plain)
        } else {
            let isCollapsed = collapsed.contains(node.id)
            Button { toggle(node.id) } label: {
                HStack(spacing: 5) {
                    Image(systemName: isCollapsed ? "chevron.right" : "chevron.down")
                        .font(.system(size: 8.5, weight: .semibold)).foregroundStyle(Theme.dim)
                        .frame(width: 10)
                    Image(systemName: "folder.fill").font(.system(size: 10.5)).foregroundStyle(Theme.fgMuted)
                    Text(node.name).font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.fgMuted)
                        .lineLimit(1).truncationMode(.head).layoutPriority(1)
                    Spacer(minLength: 4)
                }
                .padding(.leading, indent(depth)).padding(.horizontal, 8).padding(.vertical, 4)
                .contentShape(Rectangle())
                .tooltip(node.name)
            }.buttonStyle(.plain)
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
                    centered(emptyLabel)
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
                                } else if splitDiff {
                                    DiffFileView(file: f)
                                } else {
                                    NativeDiffView(file: f, isDark: theme.mode == .dark)
                                }
                            } else {
                                centered("Select a file")
                            }
                        }
                    }
                }
            } else {
                centered(loadingLabel)
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

struct DiffFileView: View {
    @EnvironmentObject var theme: ThemeManager
    let file: DiffFile
    @State private var rows: [SplitRow] = []
    @State private var maxChars: Int = 0
    private let rowH: CGFloat = 17
    private let charW: CGFloat = 6.7   // SF Mono 11pt advance; over-allocate to avoid clipping

    var body: some View {
        Group {
            if file.binary {
                Text("Binary file — no textual diff").font(.system(size: 12)).foregroundStyle(Theme.dim)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                GeometryReader { geo in
                    let viewportSide = max(160, (geo.size.width - 1) / 2)
                    // Grow each side to the longest line so nothing is truncated; the
                    // pane scrolls horizontally instead (VSCode/Zed behaviour).
                    let sideW = max(viewportSide, 68 + CGFloat(maxChars) * charW)
                    ScrollView([.vertical, .horizontal]) {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(rows) { row($0, sideW: sideW) }
                        }
                    }
                }
            }
        }
        .task(id: file.path) {
            let f = file
            let built = await Task.detached(priority: .userInitiated) { splitRows(f) }.value
            rows = built
            maxChars = built.reduce(0) { max($0, max($1.left?.count ?? 0, $1.right?.count ?? 0)) }
        }
    }

    @ViewBuilder private func row(_ r: SplitRow, sideW: CGFloat) -> some View {
        if let h = r.hunk {
            Text(h).font(Theme.mono(10)).foregroundStyle(Theme.accent).lineLimit(1)
                .padding(.horizontal, 10)
                .frame(minWidth: sideW * 2 + 1, minHeight: rowH, alignment: .leading)
                .background(Theme.accent.opacity(0.08))
        } else {
            HStack(spacing: 0) {
                side(num: r.leftN, text: r.left, hi: r.leftHi, spans: r.leftSpans, w: sideW, tint: r.changed ? Theme.danger : nil)
                Rectangle().fill(Theme.borderSoft).frame(width: 1, height: rowH)
                side(num: r.rightN, text: r.right, hi: r.rightHi, spans: r.rightSpans, w: sideW, tint: r.changed ? Theme.ok : nil)
            }
            .frame(height: rowH)
        }
    }

    private func side(num: Int?, text: String?, hi: Range<Int>?, spans: [SynSpan], w: CGFloat, tint: Color?) -> some View {
        HStack(spacing: 0) {
            Text(num.map(String.init) ?? "").font(Theme.mono(9.5)).foregroundStyle(Theme.dim)
                .frame(width: 38, alignment: .trailing).padding(.trailing, 6)
            code(text, hi: hi, spans: spans, tint: tint)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.leading, 4)
        }
        .frame(width: w, height: rowH, alignment: .leading)
        .background(text == nil ? Theme.dim.opacity(0.05) : (tint?.opacity(0.12) ?? .clear))
    }

    private func synColor(_ k: SynKind) -> Color {
        switch k {
        case .keyword: return Theme.accent
        case .string:  return Theme.ok
        case .number:  return Theme.warn
        case .comment: return Theme.dim
        case .plain:   return Theme.fgSoft
        }
    }

    private func code(_ text: String?, hi: Range<Int>?, spans: [SynSpan], tint: Color?) -> Text {
        guard let text, !text.isEmpty else { return Text(" ").font(Theme.mono(11)) }
        var a = AttributedString(text)
        a.foregroundColor = Theme.fgSoft
        let n = text.count
        func idx(_ o: Int) -> AttributedString.Index { a.characters.index(a.startIndex, offsetBy: min(max(o, 0), n)) }
        for sp in spans where sp.kind != .plain {
            a[idx(sp.lo)..<idx(sp.hi)].foregroundColor = synColor(sp.kind)
        }
        if let hi, let tint, !hi.isEmpty {
            a[idx(hi.lowerBound)..<idx(hi.upperBound)].backgroundColor = tint.opacity(0.35)
        }
        return Text(a).font(Theme.mono(11))
    }
}

struct UnifiedRow: Identifiable, Sendable {
    let id: Int
    var hunk: String? = nil
    var oldN: Int? = nil
    var newN: Int? = nil
    var kind: DiffLine.Kind = .context
    var text: String = ""
    var spans: [SynSpan] = []
}

func unifiedRows(_ file: DiffFile) -> [UnifiedRow] {
    file.lines.map { l in
        UnifiedRow(id: l.id, hunk: l.kind == .hunk ? l.text : nil,
                   oldN: l.oldN, newN: l.newN, kind: l.kind, text: l.text,
                   spans: l.kind == .hunk ? [] : Syntax.spans(l.text))
    }
}

struct DiffUnifiedView: View {
    @EnvironmentObject var theme: ThemeManager
    let file: DiffFile
    @State private var rows: [UnifiedRow] = []
    private let rowH: CGFloat = 17

    var body: some View {
        Group {
            if file.binary {
                Text("Binary file — no textual diff").font(.system(size: 12)).foregroundStyle(Theme.dim)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView(.vertical) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(rows) { row($0) }
                    }
                }
            }
        }
        .task(id: file.path) {
            let f = file
            rows = await Task.detached(priority: .userInitiated) { unifiedRows(f) }.value
        }
    }

    @ViewBuilder private func row(_ r: UnifiedRow) -> some View {
        if let h = r.hunk {
            Text(h).font(Theme.mono(10)).foregroundStyle(Theme.accent).lineLimit(1)
                .padding(.horizontal, 10)
                .frame(maxWidth: .infinity, minHeight: rowH, alignment: .leading)
                .background(Theme.accent.opacity(0.08))
        } else {
            HStack(spacing: 0) {
                gutter(r.oldN); gutter(r.newN)
                Text(r.kind == .add ? "+" : r.kind == .del ? "-" : " ")
                    .font(Theme.mono(11)).foregroundStyle(r.kind == .add ? Theme.ok : r.kind == .del ? Theme.danger : Theme.dim)
                    .frame(width: 14, alignment: .center)
                code(r.text, spans: r.spans)
                    .lineLimit(1).truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.leading, 2)
            }
            .frame(height: rowH)
            .background(r.kind == .add ? Theme.ok.opacity(0.10) : r.kind == .del ? Theme.danger.opacity(0.10) : .clear)
        }
    }

    private func gutter(_ n: Int?) -> some View {
        Text(n.map(String.init) ?? "").font(Theme.mono(9.5)).foregroundStyle(Theme.dim)
            .frame(width: 40, alignment: .trailing).padding(.trailing, 6)
    }

    private func code(_ text: String, spans: [SynSpan]) -> Text {
        guard !text.isEmpty else { return Text(" ").font(Theme.mono(11)) }
        var a = AttributedString(text)
        a.foregroundColor = Theme.fgSoft
        let n = text.count
        func idx(_ o: Int) -> AttributedString.Index { a.characters.index(a.startIndex, offsetBy: min(max(o, 0), n)) }
        for sp in spans where sp.kind != .plain {
            let c: Color = sp.kind == .keyword ? Theme.accent : sp.kind == .string ? Theme.ok : sp.kind == .number ? Theme.warn : Theme.dim
            a[idx(sp.lo)..<idx(sp.hi)].foregroundColor = c
        }
        return Text(a).font(Theme.mono(11))
    }
}


import AppKit

struct NativeDiffView: NSViewRepresentable {
    let file: DiffFile
    var isDark: Bool

    func makeCoordinator() -> Coord { Coord() }

    func makeNSView(context: Context) -> NSScrollView {
        let tv = DiffTextView()
        tv.isEditable = false
        tv.isSelectable = true
        tv.isRichText = false
        tv.drawsBackground = false
        tv.textContainerInset = NSSize(width: 0, height: 6)
        // Grow to the widest line instead of tracking the viewport, so long lines
        // scroll horizontally rather than getting clipped (VSCode/Zed behaviour).
        tv.isHorizontallyResizable = true
        tv.isVerticallyResizable = true
        tv.minSize = .zero
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.autoresizingMask = []
        tv.textContainer?.widthTracksTextView = false
        tv.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        context.coordinator.textView = tv

        let scroll = NSScrollView()
        scroll.documentView = tv
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.drawsBackground = false
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        scroll.appearance = NSAppearance(named: isDark ? .darkAqua : .aqua)
        guard let tv = context.coordinator.textView else { return }
        if context.coordinator.path == file.path { return }
        context.coordinator.path = file.path
        let built = DiffTextView.build(file)
        tv.lineKinds = built.kinds
        tv.lineStarts = built.starts
        tv.textStorage?.setAttributedString(built.string)
        if let tc = tv.textContainer { tv.layoutManager?.ensureLayout(for: tc) }
        tv.needsDisplay = true
    }

    final class Coord { var textView: DiffTextView?; var path = "" }
}

final class DiffTextView: NSTextView {
    var lineKinds: [DiffLine.Kind] = []
    var lineStarts: [Int] = []

    private static let mono = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
    private static func synColor(_ k: SynKind) -> NSColor {
        switch k {
        case .keyword: return .systemPurple
        case .string:  return .systemTeal
        case .number:  return .systemOrange
        case .comment: return .tertiaryLabelColor
        case .plain:   return .labelColor
        }
    }
    private static func gutter(_ n: Int?) -> String {
        let s = n.map(String.init) ?? ""
        return String(repeating: " ", count: max(0, 5 - s.count)) + s
    }

    static func build(_ file: DiffFile) -> (string: NSAttributedString, kinds: [DiffLine.Kind], starts: [Int]) {
        let out = NSMutableAttributedString()
        var kinds: [DiffLine.Kind] = []
        var starts: [Int] = []
        let dim = NSColor.tertiaryLabelColor, code = NSColor.labelColor
        let addC = NSColor.systemGreen, delC = NSColor.systemRed, hunkC = NSColor.systemPurple

        func append(_ s: String, _ color: NSColor) {
            out.append(NSAttributedString(string: s, attributes: [.font: mono, .foregroundColor: color]))
        }
        func appendCode(_ text: String) {
            let a = NSMutableAttributedString(string: text + "\n", attributes: [.font: mono, .foregroundColor: code])
            for sp in Syntax.spans(text) where sp.kind != .plain {
                guard let lo = text.index(text.startIndex, offsetBy: sp.lo, limitedBy: text.endIndex),
                      let hi = text.index(text.startIndex, offsetBy: sp.hi, limitedBy: text.endIndex), lo < hi else { continue }
                a.addAttribute(.foregroundColor, value: synColor(sp.kind), range: NSRange(lo..<hi, in: text))
            }
            out.append(a)
        }
        for l in file.lines {
            starts.append(out.length)
            kinds.append(l.kind)
            if l.kind == .hunk {
                append(l.text + "\n", hunkC)
                continue
            }
            append(gutter(l.oldN) + " " + gutter(l.newN) + " ", dim)
            let sign = l.kind == .add ? "+" : l.kind == .del ? "-" : " "
            append(sign + " ", l.kind == .add ? addC : l.kind == .del ? delC : dim)
            appendCode(l.text)
        }
        return (out, kinds, starts)
    }

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard let lm = layoutManager, let tc = textContainer, !lineStarts.isEmpty else { return }
        let addBg = NSColor.systemGreen.withAlphaComponent(0.12)
        let delBg = NSColor.systemRed.withAlphaComponent(0.12)
        let hunkBg = NSColor.systemPurple.withAlphaComponent(0.08)
        let inset = textContainerInset
        // Tint the whole row across the full content width — including past the
        // viewport — so a long added/deleted line stays highlighted when scrolled.
        let width = max(bounds.width, enclosingScrollView?.contentSize.width ?? 0)
        let glyphRange = lm.glyphRange(forBoundingRect: rect, in: tc)
        lm.enumerateLineFragments(forGlyphRange: glyphRange) { _, used, _, glyphR, _ in
            let charIdx = lm.characterIndexForGlyph(at: glyphR.location)
            let line = self.lineIndex(forChar: charIdx)
            guard line < self.lineKinds.count else { return }
            let color: NSColor?
            switch self.lineKinds[line] {
            case .add: color = addBg
            case .del: color = delBg
            case .hunk: color = hunkBg
            case .context: color = nil
            }
            guard let color else { return }
            color.setFill()
            NSRect(x: 0, y: used.origin.y + inset.height, width: width, height: used.height).fill()
        }
    }

    private func lineIndex(forChar c: Int) -> Int {
        var lo = 0, hi = lineStarts.count - 1, ans = 0
        while lo <= hi {
            let mid = (lo + hi) / 2
            if lineStarts[mid] <= c { ans = mid; lo = mid + 1 } else { hi = mid - 1 }
        }
        return ans
    }
}
