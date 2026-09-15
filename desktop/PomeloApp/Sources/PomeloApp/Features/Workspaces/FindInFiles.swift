import SwiftUI

struct FindLine: Decodable, Equatable {
    var line: Int
    var text: String
    var match: Bool
}

struct FindBlock: Decodable, Equatable {
    var repo: String
    var path: String
    var lines: [FindLine]
}

private struct FindResponse: Decodable {
    var blocks: [FindBlock]
    var truncated: Bool
}

// Workspace-level Cmd+Shift+F project search, styled after Zed's multibuffer results: fixed-string content search
// across every repo, results grouped by file with a header + code excerpts (matched line plus two lines of context),
// a line-number gutter, and the matched term highlighted. Enter/click opens the file and jumps to the line.
private struct FileContentResp: Decodable {
    var text: String?
    var binary: Bool?
}

struct FindInFiles: View {
    let branch: String
    let isMain: Bool
    var mode: ThemeMode = .dark
    var workspacePath: String = ""
    @Binding var query: String
    let onChoose: (WorkspaceFileEntry, Int) -> Void
    let onClose: () -> Void

    @State private var blocks: [FindBlock] = []
    @State private var colored: [Int: [NSAttributedString]] = [:]
    @State private var collapsed: Set<String> = []
    @State private var truncated = false
    @State private var index = 0
    @State private var searching = false
    @State private var searchTask: Task<Void, Never>?
    @State private var fileLines: [String: [String]] = [:]
    // Edited excerpt text (Zed-style multibuffer), keyed by block index; used to write back on Cmd+S.
    @State private var edited: [Int: [String]] = [:]
    @FocusState private var focused: Bool

    private let expandStep = 10
    private let lineHeight: CGFloat = 16
    private let codeFontSize: CGFloat = 12

    private var dirtyCount: Int { edited.count }

    // Flat list of selectable match lines as (block, row) so arrow keys skip context lines.
    private var matchPositions: [(b: Int, r: Int)] {
        var out: [(Int, Int)] = []
        for (bi, blk) in blocks.enumerated() {
            for (ri, ln) in blk.lines.enumerated() where ln.match { out.append((bi, ri)) }
        }
        return out
    }

    private var matchCount: Int { matchPositions.count }

    private func matchIndexAt(_ bi: Int, _ ri: Int) -> Int? {
        matchPositions.firstIndex(where: { $0.b == bi && $0.r == ri })
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.borderSoft)
            if blocks.isEmpty {
                VStack {
                    Text(query.trimmingCharacters(in: .whitespaces).isEmpty ? "Type to search file contents" : "No matches")
                        .font(.system(size: 12)).foregroundStyle(Theme.dim)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                results
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Theme.bg)
        .onKeyPress(.downArrow) { move(1); return .handled }
        .onKeyPress(.upArrow) { move(-1); return .handled }
        .onKeyPress(.escape) { onClose(); return .handled }
        .onChange(of: mode) { _ in recolorAll() }
        .background {
            Button("") { saveEdits() }.keyboardShortcut("s", modifiers: .command).opacity(0).allowsHitTesting(false)
        }
        .onAppear {
            focused = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { focused = true }
            if !query.trimmingCharacters(in: .whitespaces).isEmpty && blocks.isEmpty { scheduleSearch() }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "text.magnifyingglass").font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
            TextField("Search all files...", text: $query)
                .textFieldStyle(.plain).font(.system(size: 14)).foregroundStyle(Theme.fg)
                .focused($focused)
                .onChange(of: query) { _ in scheduleSearch() }
                .onSubmit { choose() }
            if dirtyCount > 0 {
                Button { saveEdits() } label: {
                    Text("Save \(dirtyCount)").font(.system(size: 11, weight: .medium))
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(Theme.accent.opacity(0.2), in: Capsule())
                        .foregroundStyle(Theme.accent)
                }.buttonStyle(.plain)
            }
            if searching {
                ProgressView().controlSize(.small)
            } else if matchCount > 0 {
                Text("\(min(index + 1, matchCount))/\(matchCount)\(truncated ? "+" : "")")
                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.dim)
                HStack(spacing: 2) {
                    Button { move(-1) } label: { Image(systemName: "chevron.up") }
                    Button { move(1) } label: { Image(systemName: "chevron.down") }
                }
                .buttonStyle(.plain).font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
    }

    // Write each edited excerpt back to its file's original line range. Verify the on-disk lines still match the
    // excerpt's original text before replacing, so a file changed since the search can't be clobbered.
    private func saveEdits() {
        guard !edited.isEmpty, !workspacePath.isEmpty else { return }
        struct Item { let bi: Int; let start: Int; let count: Int; let original: [String]; let newLines: [String] }
        var byFile: [String: (repo: String, path: String, items: [Item])] = [:]
        for (bi, newLines) in edited {
            guard bi < blocks.count, let start = blocks[bi].lines.first?.line else { continue }
            let blk = blocks[bi]
            var entry = byFile[fileKey(blk)] ?? (blk.repo, blk.path, [])
            entry.items.append(Item(bi: bi, start: start, count: blk.lines.count,
                                    original: blk.lines.map(\.text), newLines: newLines))
            byFile[fileKey(blk)] = entry
        }
        var saved = Set<Int>()
        for (_, entry) in byFile {
            let rel = entry.repo.isEmpty ? entry.path : entry.repo + "/" + entry.path
            let abs = (workspacePath as NSString).appendingPathComponent(rel)
            guard let text = try? String(contentsOfFile: abs, encoding: .utf8) else { continue }
            var fileArr = text.components(separatedBy: "\n")
            for item in entry.items.sorted(by: { $0.start > $1.start }) {
                let s = item.start - 1, e = item.start - 1 + item.count
                guard s >= 0, e <= fileArr.count, Array(fileArr[s..<e]) == item.original else { continue }
                fileArr.replaceSubrange(s..<e, with: item.newLines)
                saved.insert(item.bi)
            }
            try? fileArr.joined(separator: "\n").write(toFile: abs, atomically: true, encoding: .utf8)
        }
        for bi in saved { edited.removeValue(forKey: bi) }
        // Re-run only when everything saved (refreshes line ranges); keep any edits that failed the safety check.
        if edited.isEmpty { scheduleSearch() }
    }

    private var results: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(blocks.enumerated()), id: \.offset) { bi, blk in
                        let firstOfFile = bi == 0 || blocks[bi - 1].path != blk.path || blocks[bi - 1].repo != blk.repo
                        let hidden = collapsed.contains(fileKey(blk))
                        if firstOfFile { fileHeader(blk) }
                        if !hidden {
                            if !firstOfFile {
                                Divider().overlay(Theme.borderSoft.opacity(0.5)).padding(.leading, 54)
                            }
                            excerpt(blk, bi: bi).id("blk-\(bi)")
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .onChange(of: index) { _ in
                guard index >= 0, index < matchPositions.count else { return }
                proxy.scrollTo("blk-\(matchPositions[index].b)", anchor: .center)
            }
        }
    }

    private func fileKey(_ blk: FindBlock) -> String { blk.repo + "/" + blk.path }

    private func fileHeader(_ blk: FindBlock) -> some View {
        let dir = (blk.path as NSString).deletingLastPathComponent
        let prefix = blk.repo.isEmpty ? dir : blk.repo + (dir.isEmpty ? "" : "/" + dir)
        let key = fileKey(blk)
        let isCollapsed = collapsed.contains(key)
        return HStack(spacing: 7) {
            Image(systemName: isCollapsed ? "chevron.right" : "chevron.down")
                .font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.fgMuted)
                .frame(width: 12)
            if let mat = MaterialIcon.file(blk.path).map({ "mi-" + $0 }) {
                Image(mat, bundle: .module).resizable().aspectRatio(contentMode: .fit).frame(width: 15, height: 15)
            } else {
                Image(systemName: "doc.text").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            }
            Text((blk.path as NSString).lastPathComponent)
                .font(.system(size: 12.5, weight: .bold)).foregroundStyle(Theme.fg)
            Text(prefix).font(.system(size: 11)).foregroundStyle(Theme.dim).lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 8)
            Button { if let m = blk.lines.first(where: { $0.match }) { open(blk, m) } } label: {
                Text("Open File").font(.system(size: 10.5, weight: .medium)).foregroundStyle(Theme.fgMuted)
            }.buttonStyle(.plain)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.bgSoft)
        .overlay(alignment: .top) { Rectangle().fill(Theme.border).frame(height: 1) }
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.border).frame(height: 1) }
        .contentShape(Rectangle())
        .onTapGesture { if isCollapsed { collapsed.remove(key) } else { collapsed.insert(key) } }
    }

    // One editable excerpt (Zed multibuffer): a line-number gutter (with expand chevrons) beside an editable,
    // syntax-highlighted code view. Both use the same fixed line height so numbers stay aligned with code lines.
    private func excerpt(_ blk: FindBlock, bi: Int) -> some View {
        let canUp = (blk.lines.first?.line ?? 1) > 1
        let canDown = canExpandDown(blk)
        let last = blk.lines.count - 1
        return HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .trailing, spacing: 0) {
                ForEach(Array(blk.lines.enumerated()), id: \.offset) { ri, ln in
                    gutterRow(ln, up: ri == 0 && canUp, down: ri == last && canDown, bi: bi)
                }
            }
            .padding(.leading, 8)
            Rectangle().fill(Theme.borderSoft.opacity(0.7)).frame(width: 1)
            ExcerptEditor(lines: linesFor(blk, bi: bi), lineHeight: lineHeight, fontSize: codeFontSize,
                          textColor: NSColor(Theme.fg), onEdit: { edited[bi] = $0 })
                .padding(.leading, 4).padding(.trailing, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 2)
    }

    private func gutterRow(_ ln: FindLine, up: Bool, down: Bool, bi: Int) -> some View {
        HStack(spacing: 4) {
            Group {
                if up { Button { expand(bi: bi, up: true) } label: { Image(systemName: "chevron.up") }.buttonStyle(.plain) }
                else if down { Button { expand(bi: bi, up: false) } label: { Image(systemName: "chevron.down") }.buttonStyle(.plain) }
                else { Color.clear }
            }
            .font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.fgMuted).frame(width: 12)
            Text("\(ln.line)")
                .font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.dim)
                .frame(width: 38, alignment: .trailing)
        }
        .frame(height: lineHeight)
    }

    // Attributed lines for the editor: edited text (plain) if dirty, else the syntax-highlighted lines (or plain
    // fallback), with the query term emphasized on matched lines.
    private func linesFor(_ blk: FindBlock, bi: Int) -> [NSAttributedString] {
        if let e = edited[bi] {
            return e.map { NSAttributedString(string: $0, attributes: [.foregroundColor: NSColor(Theme.fg)]) }
        }
        let base: [NSAttributedString]
        if let c = colored[bi], c.count == blk.lines.count {
            base = c
        } else {
            base = blk.lines.map { NSAttributedString(string: $0.text, attributes: [.foregroundColor: NSColor(Theme.fg)]) }
        }
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return base }
        return zip(base, blk.lines).map { (attr, ln) in
            guard ln.match else { return attr }
            let m = NSMutableAttributedString(attributedString: attr)
            let s = m.string as NSString
            var r = s.range(of: q, options: .caseInsensitive)
            while r.location != NSNotFound {
                m.addAttribute(.backgroundColor, value: NSColor(Theme.accent.opacity(0.3)), range: r)
                let next = r.location + max(r.length, 1)
                if next >= s.length { break }
                r = s.range(of: q, options: .caseInsensitive, range: NSRange(location: next, length: s.length - next))
            }
            return m
        }
    }

    private func canExpandDown(_ blk: FindBlock) -> Bool {
        guard let last = blk.lines.last?.line else { return false }
        if let lines = fileLines[fileKey(blk)] { return last < lines.count }
        return true
    }

    private func expand(bi: Int, up: Bool) {
        guard bi >= 0, bi < blocks.count else { return }
        let blk = blocks[bi]
        let key = fileKey(blk)
        if let lines = fileLines[key] {
            applyExpand(bi: bi, up: up, lines: lines)
            return
        }
        let repo = blk.repo, path = blk.path
        Task {
            let data = await Task.detached(priority: .userInitiated) {
                FileStore.read(branch: branch, repo: repo, path: path, isMain: isMain)
            }.value
            let resp = PomJSON.decode(FileContentResp.self, from: data)
            guard let text = resp?.text, resp?.binary != true else { return }
            let lines = text.components(separatedBy: "\n")
            fileLines[key] = lines
            applyExpand(bi: bi, up: up, lines: lines)
        }
    }

    private func applyExpand(bi: Int, up: Bool, lines: [String]) {
        guard bi >= 0, bi < blocks.count else { return }
        var blk = blocks[bi]
        guard let first = blk.lines.first?.line, let last = blk.lines.last?.line else { return }
        func lineText(_ n: Int) -> String { n >= 1 && n <= lines.count ? lines[n - 1] : "" }
        if up {
            let start = max(1, first - expandStep)
            guard start < first else { return }
            let added = (start..<first).map { FindLine(line: $0, text: lineText($0), match: false) }
            blk.lines.insert(contentsOf: added, at: 0)
        } else {
            let end = min(lines.count, last + expandStep)
            guard end > last else { return }
            let added = ((last + 1)...end).map { FindLine(line: $0, text: lineText($0), match: false) }
            blk.lines.append(contentsOf: added)
        }
        blocks[bi] = blk
        normalize()
        recolorAll()
    }

    // Merge consecutive same-file excerpts whose line ranges touch or overlap into one, deduping by line number
    // (preferring the matched copy) — so expanding an excerpt into its neighbor doesn't render lines twice.
    private func normalize() {
        var out: [FindBlock] = []
        for b in blocks {
            if var last = out.last, last.repo == b.repo, last.path == b.path,
               let le = last.lines.last?.line, let bf = b.lines.first?.line, le >= bf - 1 {
                var byLine: [Int: FindLine] = [:]
                for l in last.lines { byLine[l.line] = l }
                for l in b.lines {
                    if let ex = byLine[l.line] { if l.match && !ex.match { byLine[l.line] = l } }
                    else { byLine[l.line] = l }
                }
                last.lines = byLine.values.sorted { $0.line < $1.line }
                out[out.count - 1] = last
            } else {
                out.append(b)
            }
        }
        blocks = out
    }

    private func recolorAll() {
        let theme = SQLEditor.palette(mode)
        var dict: [Int: [NSAttributedString]] = [:]
        for (i, blk) in blocks.enumerated() {
            let text = blk.lines.map(\.text).joined(separator: "\n")
            if let lines = SearchSyntax.highlightNS(blockText: text, path: blk.path, theme: theme),
               lines.count == blk.lines.count {
                dict[i] = lines
            }
        }
        colored = dict
    }

    private func move(_ delta: Int) {
        guard matchCount > 0 else { return }
        index = min(max(index + delta, 0), matchCount - 1)
    }

    private func scheduleSearch() {
        index = 0
        collapsed = []
        fileLines = [:]
        edited = [:]
        searchTask?.cancel()
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { blocks = []; colored = [:]; truncated = false; searching = false; return }
        searching = true
        searchTask = Task {
            try? await Task.sleep(nanoseconds: 200_000_000)
            if Task.isCancelled { return }
            let data = await Task.detached(priority: .userInitiated) {
                FileStore.search(branch: branch, isMain: isMain, query: q)
            }.value
            if Task.isCancelled { return }
            let r = PomJSON.decode(FindResponse.self, from: data)
            blocks = r?.blocks ?? []
            truncated = r?.truncated ?? false
            searching = false
            await highlightBlocks(blocks, mode: mode)
        }
    }

    // Syntax-color each excerpt block via tree-sitter, publishing progressively so the list paints fast.
    private func highlightBlocks(_ blks: [FindBlock], mode: ThemeMode) async {
        let theme = SQLEditor.palette(mode)
        var dict: [Int: [NSAttributedString]] = [:]
        for (i, blk) in blks.enumerated() {
            if Task.isCancelled { return }
            let text = blk.lines.map(\.text).joined(separator: "\n")
            if let lines = SearchSyntax.highlightNS(blockText: text, path: blk.path, theme: theme),
               lines.count == blk.lines.count {
                dict[i] = lines
            }
            if i % 24 == 23 { colored = dict; await Task.yield() }
        }
        if Task.isCancelled { return }
        colored = dict
    }

    private func choose() {
        guard index >= 0, index < matchPositions.count else { return }
        let p = matchPositions[index]
        open(blocks[p.b], blocks[p.b].lines[p.r])
    }

    private func open(_ blk: FindBlock, _ ln: FindLine) {
        onChoose(WorkspaceFileEntry(repo: blk.repo, path: blk.path, isDir: false), ln.line)
    }
}
