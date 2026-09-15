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
    let onChoose: (WorkspaceFileEntry, Int) -> Void
    let onClose: () -> Void

    @State private var query = ""
    @State private var blocks: [FindBlock] = []
    @State private var colored: [Int: [AttributedString]] = [:]
    @State private var collapsed: Set<String> = []
    @State private var truncated = false
    @State private var index = 0
    @State private var searching = false
    @State private var searchTask: Task<Void, Never>?
    @State private var fileLines: [String: [String]] = [:]
    @FocusState private var focused: Bool

    private let expandStep = 10

    // Flat list of selectable match lines as (block, row) so arrow keys skip context lines.
    private var matchPositions: [(b: Int, r: Int)] {
        var out: [(Int, Int)] = []
        for (bi, blk) in blocks.enumerated() {
            for (ri, ln) in blk.lines.enumerated() where ln.match { out.append((bi, ri)) }
        }
        return out
    }

    private var matchCount: Int { matchPositions.count }

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
        .onAppear {
            focused = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { focused = true }
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
                            if (blk.lines.first?.line ?? 1) > 1 {
                                expandRow(bi: bi, up: true)
                            }
                            ForEach(Array(blk.lines.enumerated()), id: \.offset) { ri, ln in
                                lineRow(ln, bi: bi, ri: ri)
                                    .id("\(bi)-\(ri)")
                                    .contentShape(Rectangle())
                                    .onTapGesture { open(blk, ln) }
                            }
                            if canExpandDown(blk) {
                                expandRow(bi: bi, up: false)
                            }
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .onChange(of: index) { _ in
                guard index >= 0, index < matchPositions.count else { return }
                let p = matchPositions[index]
                proxy.scrollTo("\(p.b)-\(p.r)", anchor: .center)
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

    private func lineRow(_ ln: FindLine, bi: Int, ri: Int) -> some View {
        let active = index >= 0 && index < matchPositions.count
            && matchPositions[index].b == bi && matchPositions[index].r == ri
        return HStack(alignment: .top, spacing: 10) {
            Text("\(ln.line)")
                .font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.dim)
                .frame(width: 44, alignment: .trailing)
            lineContent(ln, bi: bi, ri: ri)
                .font(.system(size: 12, design: .monospaced))
                .lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 0)
        }
        .padding(.trailing, 12).padding(.vertical, 1.5)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(active ? Theme.accent.opacity(0.16) : (ln.match ? Theme.accent.opacity(0.06) : .clear))
    }

    // Zed-style "expand excerpt": a thin gutter-aligned control that pulls in more surrounding context lines.
    private func expandRow(bi: Int, up: Bool) -> some View {
        Button { expand(bi: bi, up: up) } label: {
            HStack(spacing: 6) {
                Image(systemName: up ? "chevron.up" : "chevron.down")
                    .font(.system(size: 9, weight: .bold))
                    .frame(width: 44, alignment: .trailing)
                Rectangle().fill(Theme.borderSoft.opacity(0.6)).frame(height: 1)
            }
            .foregroundStyle(Theme.dim)
            .padding(.trailing, 12).padding(.vertical, 3)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
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
        var dict: [Int: [AttributedString]] = [:]
        for (i, blk) in blocks.enumerated() {
            let text = blk.lines.map(\.text).joined(separator: "\n")
            if let lines = SearchSyntax.highlight(blockText: text, path: blk.path, theme: theme),
               lines.count == blk.lines.count {
                dict[i] = lines
            }
        }
        colored = dict
    }

    // Prefer the syntax-highlighted line; fall back to plain text (dim for context) with the match term boxed.
    private func lineContent(_ ln: FindLine, bi: Int, ri: Int) -> Text {
        if let lines = colored[bi], ri < lines.count {
            var a = lines[ri]
            if ln.match { addMatchBackground(&a) }
            return Text(a)
        }
        return ln.match ? Text(highlighted(ln.text)).foregroundColor(Theme.fg)
                        : Text(ln.text).foregroundColor(Theme.dim)
    }

    private func addMatchBackground(_ a: inout AttributedString) {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return }
        var from = a.startIndex
        while from < a.endIndex, let r = a[from...].range(of: q, options: .caseInsensitive) {
            a[r].backgroundColor = Theme.accent.opacity(0.35)
            from = r.upperBound
        }
    }

    // Fallback highlight (no grammar): box the query occurrences in a plain line.
    private func highlighted(_ s: String) -> AttributedString {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return AttributedString(s) }
        var out = AttributedString("")
        var rest = Substring(s)
        while let r = rest.range(of: q, options: .caseInsensitive) {
            out += AttributedString(String(rest[rest.startIndex..<r.lowerBound]))
            var hit = AttributedString(String(rest[r]))
            hit.backgroundColor = Theme.accent.opacity(0.35)
            out += hit
            rest = rest[r.upperBound...]
        }
        out += AttributedString(String(rest))
        return out
    }

    private func move(_ delta: Int) {
        guard matchCount > 0 else { return }
        index = min(max(index + delta, 0), matchCount - 1)
    }

    private func scheduleSearch() {
        index = 0
        collapsed = []
        fileLines = [:]
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
        var dict: [Int: [AttributedString]] = [:]
        for (i, blk) in blks.enumerated() {
            if Task.isCancelled { return }
            let text = blk.lines.map(\.text).joined(separator: "\n")
            if let lines = SearchSyntax.highlight(blockText: text, path: blk.path, theme: theme),
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
