import SwiftUI
import AppKit

private enum RibbonKind: Sendable { case insert, delete, modify, hunk }

private struct RibbonMarker: Sendable { let line: Int; let color: Color }

private struct RLine: Sendable {
    let n: Int?
    let attr: AttributedString
    let kind: RibbonKind?
}

private struct RibbonBlock: Sendable {
    let kind: RibbonKind
    let left: Range<Int>
    let right: Range<Int>
}

private struct RibbonModel: Sendable {
    var left: [RLine] = []
    var right: [RLine] = []
    var blocks: [RibbonBlock] = []
    var leftMarkers: [RibbonMarker] = []
    var rightMarkers: [RibbonMarker] = []
    var maxLeftChars = 0
    var maxRightChars = 0
}

private struct AlignmentMap: Sendable {
    struct Segment: Sendable {
        let uStart: CGFloat, uLen: CGFloat
        let lStart: CGFloat, lLen: CGFloat
        let rStart: CGFloat, rLen: CGFloat
    }
    private(set) var segments: [Segment] = []
    private(set) var totalUnified: CGFloat = 0

    init() {}

    init(blocks: [RibbonBlock], leftCount: Int, rightCount: Int) {
        var u: CGFloat = 0, l = 0, r = 0
        func equalRun(toLeft target: Int) {
            let n = target - l
            guard n > 0 else { return }
            segments.append(Segment(uStart: u, uLen: CGFloat(n),
                                    lStart: CGFloat(l), lLen: CGFloat(n),
                                    rStart: CGFloat(r), rLen: CGFloat(n)))
            u += CGFloat(n); l += n; r += n
        }
        for b in blocks {
            equalRun(toLeft: b.left.lowerBound)
            let ln = b.left.count, rn = b.right.count
            let un = CGFloat(max(ln, rn))
            segments.append(Segment(uStart: u, uLen: un,
                                    lStart: CGFloat(l), lLen: CGFloat(ln),
                                    rStart: CGFloat(r), rLen: CGFloat(rn)))
            u += un; l += ln; r += rn
        }
        equalRun(toLeft: leftCount)
        totalUnified = u
    }

    func tops(atUnified u: CGFloat) -> (left: CGFloat, right: CGFloat) {
        guard !segments.isEmpty else { return (u, u) }
        if u <= 0 { return (0, 0) }
        var lo = 0, hi = segments.count - 1, found = 0
        while lo <= hi {
            let mid = (lo + hi) / 2
            if segments[mid].uStart <= u { found = mid; lo = mid + 1 } else { hi = mid - 1 }
        }
        let s = segments[found]
        let t = s.uLen > 0 ? min(max((u - s.uStart) / s.uLen, 0), 1) : 0
        return (s.lStart + t * s.lLen, s.rStart + t * s.rLen)
    }
}

private func ribbonMiddleDiff(_ a: String, _ b: String) -> (Range<Int>, Range<Int>) {
    let ac = Array(a), bc = Array(b)
    var p = 0
    while p < ac.count && p < bc.count && ac[p] == bc[p] { p += 1 }
    var s = 0
    while s < ac.count - p && s < bc.count - p && ac[ac.count - 1 - s] == bc[bc.count - 1 - s] { s += 1 }
    return (p..<(ac.count - s), p..<(bc.count - s))
}

private func ribbonSimilarity(_ a: String, _ b: String) -> Double {
    if a == b { return 1 }
    let ac = Array(a), bc = Array(b)
    var p = 0
    while p < ac.count && p < bc.count && ac[p] == bc[p] { p += 1 }
    var s = 0
    while s < ac.count - p && s < bc.count - p && ac[ac.count - 1 - s] == bc[bc.count - 1 - s] { s += 1 }
    let maxLen = max(ac.count, bc.count)
    return maxLen == 0 ? 1 : Double(p + s) / Double(maxLen)
}

private func ribbonAttr(_ text: String, spans: [SynSpan], wordHi: Range<Int>?, kind: RibbonKind?) -> AttributedString {
    var a = AttributedString(text)
    a.foregroundColor = kind == .hunk ? Theme.wsAccent : Theme.fgSoft
    let n = text.count
    guard n > 0 else { return a }
    func idx(_ o: Int) -> AttributedString.Index { a.characters.index(a.startIndex, offsetBy: min(max(o, 0), n)) }
    for sp in spans where sp.kind != .plain {
        a[idx(sp.lo)..<idx(sp.hi)].foregroundColor = SyntaxStyle.color(sp.kind)
    }
    if let w = wordHi, !w.isEmpty {
        a[idx(w.lowerBound)..<idx(w.upperBound)].backgroundColor = RibbonPalette.word(kind)
    }
    return a
}

private func buildRibbonModel(_ file: DiffFile) -> RibbonModel {
    var m = RibbonModel()
    var dels: [DiffLine] = [], adds: [DiffLine] = []

    func addLeft(_ n: Int?, _ text: String, _ spans: [SynSpan], _ wordHi: Range<Int>?, _ kind: RibbonKind?) {
        m.maxLeftChars = max(m.maxLeftChars, text.count)
        m.left.append(RLine(n: n, attr: ribbonAttr(text, spans: spans, wordHi: wordHi, kind: kind), kind: kind))
    }
    func addRight(_ n: Int?, _ text: String, _ spans: [SynSpan], _ wordHi: Range<Int>?, _ kind: RibbonKind?) {
        m.maxRightChars = max(m.maxRightChars, text.count)
        m.right.append(RLine(n: n, attr: ribbonAttr(text, spans: spans, wordHi: wordHi, kind: kind), kind: kind))
    }

    func flush() {
        guard !dels.isEmpty || !adds.isEmpty else { return }
        let lStart = m.left.count, rStart = m.right.count
        var matchAdd = [Int?](repeating: nil, count: dels.count)
        var delOfAdd = [Int?](repeating: nil, count: adds.count)
        if !dels.isEmpty, !adds.isEmpty, dels.count * adds.count <= 2000 {
            var used = [Bool](repeating: false, count: adds.count)
            for i in dels.indices {
                var bestJ = -1, bestS = 0.34
                for j in adds.indices where !used[j] {
                    let s = ribbonSimilarity(dels[i].text, adds[j].text)
                    if s > bestS { bestS = s; bestJ = j }
                }
                if bestJ >= 0 { matchAdd[i] = bestJ; delOfAdd[bestJ] = i; used[bestJ] = true }
            }
        }
        let anyMatch = matchAdd.contains { $0 != nil }
        let blockKind: RibbonKind = anyMatch ? .modify : (adds.isEmpty ? .delete : (dels.isEmpty ? .insert : .modify))

        for (i, d) in dels.enumerated() {
            if let j = matchAdd[i] {
                var wr: Range<Int>?
                if d.text != adds[j].text, d.text.count < 400, adds[j].text.count < 400 {
                    wr = ribbonMiddleDiff(d.text, adds[j].text).0
                }
                addLeft(d.oldN, d.text, Syntax.spans(d.text), wr, .modify)
            } else {
                addLeft(d.oldN, d.text, Syntax.spans(d.text), nil, .delete)
            }
        }
        for (j, a) in adds.enumerated() {
            if let i = delOfAdd[j] {
                var wr: Range<Int>?
                if a.text != dels[i].text, a.text.count < 400, dels[i].text.count < 400 {
                    wr = ribbonMiddleDiff(dels[i].text, a.text).1
                }
                addRight(a.newN, a.text, Syntax.spans(a.text), wr, .modify)
            } else {
                addRight(a.newN, a.text, Syntax.spans(a.text), nil, .insert)
            }
        }
        m.blocks.append(RibbonBlock(kind: blockKind, left: lStart..<m.left.count, right: rStart..<m.right.count))
        dels.removeAll(); adds.removeAll()
    }

    for l in file.lines {
        switch l.kind {
        case .hunk:
            flush()
            addLeft(nil, l.text, [], nil, .hunk)
            addRight(nil, "", [], nil, .hunk)
        case .del: dels.append(l)
        case .add: adds.append(l)
        case .context:
            flush()
            let sp = Syntax.spans(l.text)
            addLeft(l.oldN, l.text, sp, nil, nil)
            addRight(l.newN, l.text, sp, nil, nil)
        }
    }
    flush()
    for b in m.blocks where b.left.isEmpty {
        m.leftMarkers.append(RibbonMarker(line: b.left.lowerBound, color: RibbonPalette.ribbon(b.kind)))
    }
    for b in m.blocks where b.right.isEmpty {
        m.rightMarkers.append(RibbonMarker(line: b.right.lowerBound, color: RibbonPalette.ribbon(b.kind)))
    }
    return m
}

private enum RibbonPalette {
    static func hue(_ k: RibbonKind?) -> Color {
        switch k {
        case .insert: return Theme.ok
        case .delete: return Theme.danger
        case .modify: return Theme.accent
        case .hunk:   return Theme.wsAccent
        case nil:     return .clear
        }
    }
    static func rowBg(_ k: RibbonKind?) -> Color {
        switch k {
        case nil:   return .clear
        case .hunk: return Theme.wsAccent.opacity(0.10)
        default:    return hue(k).opacity(0.15)
        }
    }
    static func word(_ k: RibbonKind?) -> Color { hue(k).opacity(0.42) }
    static func ribbon(_ k: RibbonKind) -> Color { hue(k).opacity(0.20) }
}

private let ribbonLineHeight: CGFloat = 18
private let ribbonNumberWidth: CGFloat = 40
private let ribbonGutterWidth: CGFloat = 44
private let ribbonTopBuffer: CGFloat = ribbonLineHeight * 2
private let ribbonCharWidth: CGFloat = 6.62

private func ribbonRow(_ l: RLine, hOffset: CGFloat, codeWidth: CGFloat) -> some View {
    HStack(spacing: 0) {
        Text(l.n.map(String.init) ?? "")
            .font(Theme.mono(10)).foregroundStyle(Theme.dim)
            .frame(width: ribbonNumberWidth, alignment: .trailing)
            .padding(.trailing, 6)
        Text(l.attr).font(Theme.mono(11))
            .lineLimit(1).fixedSize(horizontal: true, vertical: false)
            .padding(.trailing, 8)
            .offset(x: -hOffset)
            .frame(width: max(0, codeWidth), alignment: .leading)
            .clipped()
        Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: ribbonLineHeight, maxHeight: ribbonLineHeight, alignment: .leading)
    .background(RibbonPalette.rowBg(l.kind))
}

private struct RibbonPane: View {
    let lines: [RLine]
    let topLine: CGFloat
    let viewportH: CGFloat
    let paneWidth: CGFloat
    let hOffset: CGFloat
    let markers: [RibbonMarker]

    var body: some View {
        let codeWidth = max(0, paneWidth - ribbonNumberWidth - 6 - 8)
        let top = Int(topLine.rounded(.down))
        let bufRows = Int((ribbonTopBuffer / ribbonLineHeight).rounded(.up)) + 1
        let first = max(0, top - bufRows)
        let last = min(lines.count, top + Int(ceil(viewportH / ribbonLineHeight)) + 2)
        VStack(spacing: 0) {
            ForEach(first..<max(first, last), id: \.self) { i in
                ribbonRow(lines[i], hOffset: hOffset, codeWidth: codeWidth)
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .offset(y: (CGFloat(first) - topLine) * ribbonLineHeight + ribbonTopBuffer)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .overlay(alignment: .topLeading) {
            Canvas { ctx, size in
                for mk in markers {
                    let y = (CGFloat(mk.line) - topLine) * ribbonLineHeight - 1 + ribbonTopBuffer
                    if y < -2 || y > size.height { continue }
                    ctx.fill(Path(CGRect(x: 0, y: y, width: size.width, height: 1)), with: .color(mk.color))
                }
            }
            .allowsHitTesting(false)
        }
        .clipped()
    }
}

private struct RibbonGutter: View {
    let blocks: [RibbonBlock]
    let leftTop: CGFloat
    let rightTop: CGFloat
    let viewportH: CGFloat

    var body: some View {
        Canvas { ctx, size in
            let x0: CGFloat = 0, x1 = size.width, mid = size.width / 2
            func ly(_ line: Int) -> CGFloat { (CGFloat(line) - leftTop) * ribbonLineHeight + ribbonTopBuffer }
            func ry(_ line: Int) -> CGFloat { (CGFloat(line) - rightTop) * ribbonLineHeight + ribbonTopBuffer }
            for b in blocks where b.kind != .hunk {
                let lTop = ly(b.left.lowerBound), lBot = ly(b.left.upperBound)
                let rTop = ry(b.right.lowerBound), rBot = ry(b.right.upperBound)
                if max(lBot, rBot) < -40 || min(lTop, rTop) > size.height + 40 { continue }
                var path = Path()
                path.move(to: CGPoint(x: x0, y: lTop))
                path.addCurve(to: CGPoint(x: x1, y: rTop),
                              control1: CGPoint(x: mid, y: lTop), control2: CGPoint(x: mid, y: rTop))
                path.addLine(to: CGPoint(x: x1, y: rBot))
                path.addCurve(to: CGPoint(x: x0, y: lBot),
                              control1: CGPoint(x: mid, y: rBot), control2: CGPoint(x: mid, y: lBot))
                path.closeSubpath()
                ctx.fill(path, with: .color(RibbonPalette.ribbon(b.kind)))
            }
        }
        .frame(width: ribbonGutterWidth, height: viewportH)
        .clipped()
    }
}

private struct ScrollWheelCatcher: NSViewRepresentable {
    var onScroll: (_ dx: CGFloat, _ dy: CGFloat, _ atX: CGFloat) -> Void
    func makeNSView(context: Context) -> CatcherView { let v = CatcherView(); v.onScroll = onScroll; return v }
    func updateNSView(_ v: CatcherView, context: Context) { v.onScroll = onScroll }

    final class CatcherView: NSView {
        var onScroll: ((CGFloat, CGFloat, CGFloat) -> Void)?
        override func scrollWheel(with event: NSEvent) {
            let scale = event.hasPreciseScrollingDeltas ? 1 : ribbonLineHeight
            let loc = convert(event.locationInWindow, from: nil)
            onScroll?(event.scrollingDeltaX * scale, event.scrollingDeltaY * scale, loc.x)
        }
        override func hitTest(_ point: NSPoint) -> NSView? {
            if let e = NSApp.currentEvent, e.type == .scrollWheel { return self }
            return nil
        }
    }
}

struct SplitDiffRibbon: View {
    let file: DiffFile
    var isDark: Bool

    @State private var model = RibbonModel()
    @State private var map = AlignmentMap()
    @State private var builtKey = ""
    @State private var unified: CGFloat = 0
    @State private var hScroll: CGFloat = 0

    var body: some View {
        GeometryReader { geo in
            let vh = geo.size.height
            let paneW = max(0, (geo.size.width - ribbonGutterWidth) / 2)
            let codeW = max(0, paneW - ribbonNumberWidth - 6 - 8)
            let tops = map.tops(atUnified: unified)
            let maxH = max(0, CGFloat(max(model.maxLeftChars, model.maxRightChars)) * ribbonCharWidth - codeW + 24)
            HStack(alignment: .top, spacing: 0) {
                RibbonPane(lines: model.left, topLine: tops.left, viewportH: vh, paneWidth: paneW,
                           hOffset: hScroll, markers: model.leftMarkers)
                    .frame(width: paneW)
                RibbonGutter(blocks: model.blocks, leftTop: tops.left, rightTop: tops.right, viewportH: vh)
                    .overlay(Rectangle().fill(Theme.borderSoft).frame(width: 1), alignment: .leading)
                    .overlay(Rectangle().fill(Theme.borderSoft).frame(width: 1), alignment: .trailing)
                RibbonPane(lines: model.right, topLine: tops.right, viewportH: vh, paneWidth: paneW,
                           hOffset: hScroll, markers: model.rightMarkers)
                    .frame(width: paneW)
            }
            .frame(width: geo.size.width, height: vh, alignment: .topLeading)
            .clipped()
            .overlay(ScrollWheelCatcher { dx, dy, _ in
                let visible = vh / ribbonLineHeight
                let maxU = max(0, map.totalUnified - visible + 1 + ribbonTopBuffer / ribbonLineHeight)
                unified = min(max(unified - dy / ribbonLineHeight, 0), maxU)
                if abs(dx) > abs(dy) * 0.5 {
                    hScroll = min(max(hScroll - dx, 0), maxH)
                }
            })
        }
        .background(Theme.bg)
        .onAppear { rebuild() }
        .onChange(of: file.path) { rebuild() }
        .onChange(of: isDark) { rebuild(force: true) }
    }

    private func rebuild(force: Bool = false) {
        let key = "\(file.path)|\(isDark)"
        guard force || builtKey != key else { return }
        let samePath = builtKey.hasPrefix(file.path + "|")
        builtKey = key
        if !samePath { unified = 0; hScroll = 0; model = RibbonModel(); map = AlignmentMap() }
        let f = file
        Task {
            let built = await Task.detached(priority: .userInitiated) { () -> (RibbonModel, AlignmentMap) in
                let m = buildRibbonModel(f)
                return (m, AlignmentMap(blocks: m.blocks, leftCount: m.left.count, rightCount: m.right.count))
            }.value
            if builtKey == key { model = built.0; map = built.1 }
        }
    }
}
