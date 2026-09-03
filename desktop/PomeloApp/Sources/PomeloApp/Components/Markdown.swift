import SwiftUI

struct MarkdownText: View {
    @EnvironmentObject var theme: ThemeManager
    let text: String
    // Long-form prose (the Review doc) wants bigger type + air; chat/inline keeps it compact.
    var reading = false
    init(_ text: String, reading: Bool = false) { self.text = text; self.reading = reading }

    // Both the block split and AttributedString(markdown:) are pure functions of the
    // text (inline also of the theme) and were re-run on every body eval; under a
    // conversation scroll + a streaming agent that re-parsed the same comments dozens
    // of times, stacking into multi-second main-thread stalls. Cache by input.
    private static var blockCache: [String: [Block]] = [:]
    private static var inlineCache: [String: AttributedString] = [:]
    private static func evictIfNeeded() {
        if blockCache.count > 400 { blockCache.removeAll(keepingCapacity: true) }
        if inlineCache.count > 800 { inlineCache.removeAll(keepingCapacity: true) }
    }

    private var bodySize: CGFloat { reading ? 14 : 12.5 }
    private var lineGap: CGFloat { reading ? 6 : 2 }

    var body: some View {
        let bs = blocks
        let gallery: [GalleryImage] = bs.compactMap { if case .image(let alt, let url) = $0 { return GalleryImage(url: url, alt: alt) } else { return nil } }
        VStack(alignment: .leading, spacing: reading ? 12 : 8) {
            ForEach(Array(bs.enumerated()), id: \.offset) { _, b in view(b) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .environment(\.imageGallery, gallery)
    }

    private struct ListItem { let depth: Int; let ordered: Bool; let text: String }

    private enum Block {
        case heading(Int, String)
        case paragraph(String)
        case code(String)
        case list([ListItem])
        case quote(String)
        case image(String, String)
        case table([String], [[String]])
        case rule
    }

    private var blocks: [Block] {
        if let c = Self.blockCache[text] { return c }
        PerfHUD.shared.tick("md:parse")
        var out: [Block] = []
        let lines = text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        var i = 0
        func flushPara(_ buf: inout [String]) {
            if !buf.isEmpty { out.append(.paragraph(buf.joined(separator: "\n"))); buf.removeAll() }
        }
        var para: [String] = []
        while i < lines.count {
            let l = lines[i]
            let t = l.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") {
                flushPara(&para)
                i += 1
                var code: [String] = []
                while i < lines.count, !lines[i].trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                    code.append(lines[i]); i += 1
                }
                i += 1
                out.append(.code(code.joined(separator: "\n")))
                continue
            }
            if t == "---" || t == "***" || t == "___" { flushPara(&para); out.append(.rule); i += 1; continue }
            if isTableRow(t), i + 1 < lines.count, isTableSep(lines[i + 1].trimmingCharacters(in: .whitespaces)) {
                flushPara(&para)
                let header = tableCells(t)
                i += 2
                var rows: [[String]] = []
                while i < lines.count, isTableRow(lines[i].trimmingCharacters(in: .whitespaces)) {
                    rows.append(tableCells(lines[i].trimmingCharacters(in: .whitespaces))); i += 1
                }
                out.append(.table(header, rows))
                continue
            }
            if let img = imageLine(t) { flushPara(&para); out.append(.image(img.0, img.1)); i += 1; continue }
            let htmlImgs = htmlImages(t)
            if !htmlImgs.isEmpty {
                flushPara(&para)
                for im in htmlImgs { out.append(.image(im.0, im.1)) }
                i += 1; continue
            }
            if let h = heading(t) { flushPara(&para); out.append(.heading(h.0, h.1)); i += 1; continue }
            if t.hasPrefix(">") {
                flushPara(&para)
                var q: [String] = []
                while i < lines.count, lines[i].trimmingCharacters(in: .whitespaces).hasPrefix(">") {
                    q.append(String(lines[i].trimmingCharacters(in: .whitespaces).dropFirst()).trimmingCharacters(in: .whitespaces)); i += 1
                }
                out.append(.quote(q.joined(separator: "\n")))
                continue
            }
            if isListItem(t) != nil {
                flushPara(&para)
                var raw: [(indent: Int, ordered: Bool, text: String)] = []
                while i < lines.count {
                    let line = lines[i]
                    let tt = line.trimmingCharacters(in: .whitespaces)
                    guard let item = isListItem(tt) else { break }
                    raw.append((leadingSpaces(line), orderedItem(tt) != nil, item))
                    i += 1
                }
                out.append(.list(nestList(raw)))
                continue
            }
            if t.isEmpty { flushPara(&para); i += 1; continue }
            para.append(l); i += 1
        }
        flushPara(&para)
        Self.evictIfNeeded()
        Self.blockCache[text] = out
        return out
    }

    private func heading(_ t: String) -> (Int, String)? {
        guard t.hasPrefix("#") else { return nil }
        var n = 0
        for c in t { if c == "#" { n += 1 } else { break } }
        guard n <= 6, t.count > n, t[t.index(t.startIndex, offsetBy: n)] == " " else { return nil }
        return (n, String(t.dropFirst(n + 1)))
    }
    private func htmlImages(_ t: String) -> [(String, String)] {
        guard t.contains("<img"), let re = try? NSRegularExpression(pattern: "<img\\b[^>]*?>", options: .caseInsensitive)
        else { return [] }
        let ns = t as NSString
        var out: [(String, String)] = []
        for m in re.matches(in: t, range: NSRange(location: 0, length: ns.length)) {
            let tag = ns.substring(with: m.range)
            guard let src = attr(tag, "src"), !src.isEmpty else { continue }
            out.append((attr(tag, "alt") ?? "", src))
        }
        return out
    }
    private func attr(_ tag: String, _ name: String) -> String? {
        guard let re = try? NSRegularExpression(pattern: name + "\\s*=\\s*\"([^\"]*)\"", options: .caseInsensitive) else { return nil }
        let ns = tag as NSString
        guard let m = re.firstMatch(in: tag, range: NSRange(location: 0, length: ns.length)), m.numberOfRanges > 1 else { return nil }
        return ns.substring(with: m.range(at: 1))
    }
    private func isTableRow(_ t: String) -> Bool { t.hasPrefix("|") && t.dropFirst().contains("|") }
    private func isTableSep(_ t: String) -> Bool {
        guard t.hasPrefix("|") else { return false }
        return t.allSatisfy { "|:- \t".contains($0) } && t.contains("-")
    }
    private func tableCells(_ t: String) -> [String] {
        var s = Substring(t)
        if s.hasPrefix("|") { s = s.dropFirst() }
        if s.hasSuffix("|") { s = s.dropLast() }
        return s.components(separatedBy: "|").map { $0.trimmingCharacters(in: .whitespaces) }
    }

    private func imageLine(_ t: String) -> (String, String)? {
        guard t.hasPrefix("!["), let close = t.firstIndex(of: "]"),
              t.index(after: close) < t.endIndex, t[t.index(after: close)] == "(",
              t.hasSuffix(")") else { return nil }
        let alt = String(t[t.index(t.startIndex, offsetBy: 2)..<close])
        let url = String(t[t.index(close, offsetBy: 2)..<t.index(before: t.endIndex)])
        return url.isEmpty ? nil : (alt, url)
    }

    private func leadingSpaces(_ s: String) -> Int {
        var n = 0
        for c in s { if c == " " { n += 1 } else if c == "\t" { n += 4 } else { break } }
        return n
    }

    private func nestList(_ raw: [(indent: Int, ordered: Bool, text: String)]) -> [ListItem] {
        guard !raw.isEmpty else { return [] }
        let base = raw.map(\.indent).min() ?? 0
        let steps = raw.map { $0.indent - base }.filter { $0 > 0 }
        let stride = steps.min() ?? 1
        return raw.map { ListItem(depth: ($0.indent - base) / max(stride, 1), ordered: $0.ordered, text: $0.text) }
    }

    private func orderedItem(_ t: String) -> String? {
        let parts = t.prefix(while: { $0 != " " })
        if let dot = parts.last, dot == ".", parts.dropLast().allSatisfy(\.isNumber), !parts.isEmpty {
            return String(t.dropFirst(parts.count + 1))
        }
        return nil
    }
    private func isListItem(_ t: String) -> String? {
        if t.hasPrefix("- ") || t.hasPrefix("* ") || t.hasPrefix("+ ") { return String(t.dropFirst(2)) }
        return orderedItem(t)
    }

    private func inline(_ s: String) -> AttributedString {
        let key = "\(activeThemeMode.rawValue)\u{0}\(s)"
        if let c = Self.inlineCache[key] { return c }
        PerfHUD.shared.tick("md:inline")
        guard var a = try? AttributedString(markdown: s, options: .init(
            allowsExtendedAttributes: true,
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible)) else { return AttributedString(s) }
        for run in a.runs where run.inlinePresentationIntent?.contains(.code) == true {
            a[run.range].font = .system(size: 12, weight: .regular, design: .monospaced)
            a[run.range].backgroundColor = Theme.chip
            a[run.range].foregroundColor = Theme.fg
        }
        Self.evictIfNeeded()
        Self.inlineCache[key] = a
        return a
    }

    private func task(_ s: String) -> (Bool, String)? {
        if s.hasPrefix("[ ] ") { return (false, String(s.dropFirst(4))) }
        if s.hasPrefix("[x] ") || s.hasPrefix("[X] ") { return (true, String(s.dropFirst(4))) }
        return nil
    }

    private func markers(_ items: [ListItem]) -> [(item: ListItem, marker: String)] {
        var counters: [Int] = []
        return items.map { it in
            if it.depth >= counters.count { counters += Array(repeating: 0, count: it.depth - counters.count + 1) }
            counters[it.depth] += 1
            for d in (it.depth + 1)..<counters.count where d < counters.count { counters[d] = 0 }
            let n = counters[it.depth]
            let marker: String
            if !it.ordered {
                marker = ["•", "◦", "▪"][it.depth % 3]
            } else {
                switch it.depth % 3 {
                case 1: marker = lowerAlpha(n) + "."
                case 2: marker = lowerRoman(n) + "."
                default: marker = "\(n)."
                }
            }
            return (it, marker)
        }
    }

    private func lowerAlpha(_ n: Int) -> String {
        guard n >= 1 else { return "\(n)" }
        var v = n, s = ""
        while v > 0 { let r = (v - 1) % 26; s = String(UnicodeScalar(97 + r)!) + s; v = (v - 1) / 26 }
        return s
    }
    private func lowerRoman(_ n: Int) -> String {
        guard n >= 1, n < 4000 else { return "\(n)" }
        let vals = [(1000,"m"),(900,"cm"),(500,"d"),(400,"cd"),(100,"c"),(90,"xc"),(50,"l"),(40,"xl"),(10,"x"),(9,"ix"),(5,"v"),(4,"iv"),(1,"i")]
        var v = n, s = ""
        for (val, sym) in vals { while v >= val { s += sym; v -= val } }
        return s
    }

    @ViewBuilder private func view(_ b: Block) -> some View {
        switch b {
        case .heading(let n, let s):
            Text(inline(s))
                .font(.system(size: reading ? (n <= 1 ? 20 : n == 2 ? 16.5 : 14.5) : (n <= 1 ? 17 : n == 2 ? 15 : 13.5),
                              weight: .semibold))
                .foregroundStyle(Theme.fg)
                .padding(.top, reading ? 6 : 2)
        case .paragraph(let s):
            Text(inline(s)).font(.system(size: bodySize, weight: .regular)).foregroundStyle(Theme.fgSoft)
                .lineSpacing(lineGap)
                .tint(Theme.accent).textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        case .code(let s):
            ScrollView(.horizontal, showsIndicators: true) {
                Text(s).font(Theme.mono(11.5)).foregroundStyle(Theme.fg)
                    .textSelection(.enabled).fixedSize(horizontal: true, vertical: false)
                    .padding(10)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
        case .list(let items):
            VStack(alignment: .leading, spacing: reading ? 6 : 4) {
                ForEach(Array(markers(items).enumerated()), id: \.offset) { _, row in
                    HStack(alignment: .top, spacing: 8) {
                        if let (checked, _) = task(row.item.text) {
                            Image(systemName: checked ? "checkmark.square.fill" : "square")
                                .font(.system(size: 12)).foregroundStyle(checked ? Theme.accent : Theme.dim)
                                .frame(width: 16, alignment: .center)
                        } else {
                            Text(row.marker).font(.system(size: bodySize)).foregroundStyle(Theme.dim)
                                .frame(width: 20, alignment: .trailing)
                        }
                        Text(inline(task(row.item.text)?.1 ?? row.item.text)).font(.system(size: bodySize, weight: .regular))
                            .lineSpacing(lineGap)
                            .foregroundStyle(Theme.fgSoft).tint(Theme.accent).fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(.leading, CGFloat(row.item.depth) * 18)
                }
            }
        case .quote(let s):
            HStack(spacing: 8) {
                Rectangle().fill(Theme.borderSoft).frame(width: 3)
                Text(inline(s)).font(.system(size: 12.5)).foregroundStyle(Theme.fgMuted).italic()
            }.fixedSize(horizontal: false, vertical: true)
        case .image(let alt, let url):
            MarkdownImage(url: url, alt: alt)
        case .table(let header, let rows):
            tableView(header, rows)
        case .rule:
            Rectangle().fill(Theme.borderSoft).frame(height: 1).padding(.vertical, 2)
        }
    }
}

extension MarkdownText {
    @ViewBuilder func tableView(_ header: [String], _ rows: [[String]]) -> some View {
        let cols = max(header.count, rows.map(\.count).max() ?? 0)
        ScrollView(.horizontal, showsIndicators: true) {
            Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
                GridRow {
                    ForEach(0..<cols, id: \.self) { c in
                        tableCell(c < header.count ? header[c] : "", header: true)
                    }
                }
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    GridRow {
                        ForEach(0..<cols, id: \.self) { c in
                            tableCell(c < row.count ? row[c] : "", header: false)
                        }
                    }
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.borderSoft))
            .padding(1)
        }
    }

    @ViewBuilder private func tableCell(_ raw: String, header: Bool) -> some View {
        let imgs = htmlImages(raw)
        Group {
            if let first = imgs.first {
                MarkdownImage(url: first.1, alt: first.0).frame(maxWidth: 130)
            } else if !raw.isEmpty {
                Text(inline(raw)).font(.system(size: 11.5, weight: header ? .semibold : .regular))
                    .foregroundStyle(header ? Theme.fg : Theme.fgSoft)
                    .multilineTextAlignment(header ? .center : .leading)
                    .frame(maxWidth: .infinity, alignment: header ? .center : .leading)
            } else {
                Color.clear.frame(height: 1)
            }
        }
        .padding(.horizontal, 8).padding(.vertical, 6)
        .frame(width: 168, alignment: .topLeading)
        .background(header ? Theme.panel3 : Color.clear)
        .overlay(Rectangle().strokeBorder(Theme.borderSoft, lineWidth: 0.5))
    }
}

struct MarkdownImage: View {
    let url: String
    var alt: String = ""
    @Environment(\.imageGallery) private var gallery
    @State private var image: NSImage?
    @State private var failed = false
    @State private var hovered = false

    var body: some View {
        Group {
            if let img = image {
                Image(nsImage: img).resizable().aspectRatio(contentMode: .fit)
                    .frame(maxWidth: 520, alignment: .leading)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(hovered ? Theme.accent.opacity(0.6) : Theme.borderSoft))
                    .contentShape(Rectangle())
                    .onHover { hovered = $0; if $0 { NSCursor.pointingHand.push() } else { NSCursor.pop() } }
                    .onTapGesture {
                        let g = gallery.isEmpty ? [GalleryImage(url: url, alt: alt)] : gallery
                        ImagePreviewState.shared.show(g, at: g.firstIndex { $0.url == url } ?? 0)
                    }
                    .help("Click to preview")
            } else if failed {
                Label(alt.isEmpty ? "image unavailable" : alt, systemImage: "photo")
                    .font(.system(size: 11)).foregroundStyle(Theme.dim)
            } else {
                HStack(spacing: 6) { ProgressView().controlSize(.small).scaleEffect(0.6)
                    Text(alt.isEmpty ? "loading image…" : alt).font(.system(size: 11)).foregroundStyle(Theme.dim) }
            }
        }
        .task(id: url) { await load() }
    }

    private func load() async {
        image = await MarkdownImageCache.shared.image(for: url)
        if image == nil { failed = true }
    }
}
