import SwiftUI
import AppKit

// One renderer for every read-only code surface (peek, unified + split diff). The view
// is generic; a builder bakes the two things that differ — gutter text and per-line
// background — into a CodeModel.
struct CodeModel {
    let string: NSAttributedString
    let starts: [Int]
    let lineBg: [NSColor?]
}

struct CodeView: NSViewRepresentable {
    let content: String
    let lang: CodeLang
    let start: Int
    let end: Int
    var isDark: Bool
    var onSelectLines: (ClosedRange<Int>?) -> Void = { _ in }

    func makeCoordinator() -> Coord { Coord() }

    func makeNSView(context: Context) -> NSScrollView {
        let tv = CodeTextView()
        tv.configureReadOnly(inset: NSSize(width: 4, height: 8))
        tv.textContainer?.lineFragmentPadding = 0
        context.coordinator.textView = tv
        tv.delegate = context.coordinator
        return CodeTextView.makeScroll(tv)
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let ap = NSAppearance(named: isDark ? .darkAqua : .aqua)
        scroll.appearance = ap
        guard let tv = context.coordinator.textView else { return }
        tv.appearance = ap
        context.coordinator.onSelectLines = onSelectLines
        // Theme-derived colours are baked into the string at build time, so a theme
        // switch must rebuild — hence isDark in the key.
        let key = "\(content.count):\(start):\(end):\(isDark)"
        if context.coordinator.key == key { return }
        context.coordinator.key = key
        let model = CodeTextView.peek(content, lang: lang, start: start, end: end)
        tv.apply(model)
        DispatchQueue.main.async { scrollToTarget(tv) }
    }

    // Scroll below the anchor first, then to it, so it lands near the top with context;
    // a single scroll fires before AppKit settles layout and stops at the top.
    private func scrollToTarget(_ tv: CodeTextView) {
        guard start > 1, start - 1 < tv.lineStarts.count else { return }
        let hitHi = min(max(start, end), tv.lineStarts.count) - 1
        let below = min(hitHi + 10, tv.lineStarts.count - 1)
        tv.scrollRangeToVisible(NSRange(location: tv.lineStarts[below], length: 0))
        tv.scrollRangeToVisible(NSRange(location: tv.lineStarts[start - 1], length: 0))
    }

    final class Coord: NSObject, NSTextViewDelegate {
        var textView: CodeTextView?
        var key = ""
        var onSelectLines: (ClosedRange<Int>?) -> Void = { _ in }
        func textViewDidChangeSelection(_ notification: Notification) {
            guard let tv = textView, !tv.lineStarts.isEmpty else { return }
            let sel = tv.selectedRange()
            if sel.length == 0 { onSelectLines(nil); return }
            let lo = tv.lineIndex(forChar: sel.location) + 1
            let hi = tv.lineIndex(forChar: max(sel.location, sel.location + sel.length - 1)) + 1
            onSelectLines(lo...hi)
        }
    }
}

final class CodeTextView: NSTextView {
    var lineStarts: [Int] = []
    private var lineBg: [NSColor?] = []

    func apply(_ model: CodeModel) {
        lineStarts = model.starts
        lineBg = model.lineBg
        textStorage?.setAttributedString(model.string)
        if let tc = textContainer { layoutManager?.ensureLayout(for: tc) }
        needsDisplay = true
    }

    func lineIndex(forChar c: Int) -> Int {
        var lo = 0, hi = lineStarts.count - 1, ans = 0
        while lo <= hi {
            let mid = (lo + hi) / 2
            if lineStarts[mid] <= c { ans = mid; lo = mid + 1 } else { hi = mid - 1 }
        }
        return ans
    }

    func configureReadOnly(inset: NSSize) {
        isEditable = false
        isSelectable = true
        isRichText = false
        drawsBackground = false
        textContainerInset = inset
        isHorizontallyResizable = true
        isVerticallyResizable = true
        minSize = .zero
        maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        autoresizingMask = []
        textContainer?.widthTracksTextView = false
        textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
    }

    static func makeScroll(_ tv: NSTextView) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.documentView = tv
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.drawsBackground = false
        return scroll
    }

    // Fixed integer line height (min == max) so rows sit on whole pixels — a fractional
    // line height antialiases the row/highlight edges into faint stripes.
    static func paragraph(lineHeight: CGFloat) -> NSParagraphStyle {
        let p = NSMutableParagraphStyle()
        p.minimumLineHeight = lineHeight; p.maximumLineHeight = lineHeight
        return p
    }

    static func attributedLine(_ text: String, spans: [SynSpan], font: NSFont, base: NSColor,
                               paragraph: NSParagraphStyle) -> NSMutableAttributedString {
        let a = NSMutableAttributedString(string: text,
                                          attributes: [.font: font, .foregroundColor: base, .paragraphStyle: paragraph])
        for sp in spans where sp.kind != .plain {
            guard let l = text.index(text.startIndex, offsetBy: sp.lo, limitedBy: text.endIndex),
                  let h = text.index(text.startIndex, offsetBy: sp.hi, limitedBy: text.endIndex), l < h else { continue }
            a.addAttribute(.foregroundColor, value: SyntaxStyle.nsColor(sp.kind), range: NSRange(l..<h, in: text))
        }
        return a
    }

    // Fill one rect per contiguous same-colour run from its character range, not per
    // dirty band — per-band fills round differently at band edges and leave hairline seams.
    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard let lm = layoutManager, let tc = textContainer, let storage = textStorage,
              !lineStarts.isEmpty, !lineBg.isEmpty else { return }
        NSGraphicsContext.current?.shouldAntialias = false
        defer { NSGraphicsContext.current?.shouldAntialias = true }
        let inset = textContainerInset.height
        let width = max(bounds.width, enclosingScrollView?.contentSize.width ?? 0)
        var i = 0
        while i < lineBg.count {
            guard let color = lineBg[i] else { i += 1; continue }
            var j = i
            while j + 1 < lineBg.count, lineBg[j + 1] == color { j += 1 }
            let firstChar = lineStarts[i]
            let lastChar = j + 1 < lineStarts.count ? lineStarts[j + 1] : storage.length
            let glyphRange = lm.glyphRange(forCharacterRange: NSRange(location: firstChar, length: max(0, lastChar - firstChar)),
                                           actualCharacterRange: nil)
            var r = lm.boundingRect(forGlyphRange: glyphRange, in: tc)
            r.origin.y += inset
            r.origin.x = 0
            r.size.width = width
            color.setFill()
            backingAlignedRect(r, options: .alignAllEdgesOutward).fill()
            i = j + 1
        }
    }
}

extension CodeTextView {
    // Blend a tint onto the opaque editor background so a single fill reads as the
    // intended colour on any theme (no alpha doubling where runs meet).
    static func opaque(_ tint: NSColor, _ a: CGFloat) -> NSColor {
        let base = NSColor(Theme.bg).usingColorSpace(.sRGB) ?? .black
        let t = tint.usingColorSpace(.sRGB) ?? tint
        return NSColor(srgbRed: base.redComponent * (1 - a) + t.redComponent * a,
                       green: base.greenComponent * (1 - a) + t.greenComponent * a,
                       blue: base.blueComponent * (1 - a) + t.blueComponent * a, alpha: 1)
    }

    static func peek(_ content: String, lang: CodeLang, start: Int, end: Int) -> CodeModel {
        let mono = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
        let para = paragraph(lineHeight: 20)
        let lines = content.components(separatedBy: "\n")
        let spansPerLine = Syntax.tokenize(content, lang: lang)
        let gutterW = max(3, String(lines.count).count)
        let dim = NSColor.tertiaryLabelColor, code = NSColor.labelColor
        let anchor = NSColor.controlAccentColor.withAlphaComponent(0.16)
        let hitLo = start > 0 ? min(start, lines.count) - 1 : -1
        let hitHi = start > 0 ? min(max(start, end), lines.count) - 1 : -1
        let out = NSMutableAttributedString()
        var starts: [Int] = [], bg: [NSColor?] = []
        for (i, line) in lines.enumerated() {
            starts.append(out.length)
            bg.append(hitLo >= 0 && i >= hitLo && i <= hitHi ? anchor : nil)
            let num = String(i + 1)
            let pad = String(repeating: " ", count: max(0, gutterW - num.count))
            out.append(NSAttributedString(string: pad + num + "  ", attributes: [.font: mono, .foregroundColor: dim, .paragraphStyle: para]))
            let spans = i < spansPerLine.count ? spansPerLine[i] : []
            out.append(attributedLine(line + "\n", spans: spans, font: mono, base: code, paragraph: para))
        }
        return CodeModel(string: out, starts: starts, lineBg: bg)
    }

    enum Side { case left, right }

    // Rows are paired upstream (splitRows) so both columns align line-for-line; a nil
    // cell renders as a blank padded row.
    static func splitSide(_ rows: [SplitRow], side: Side) -> CodeModel {
        let mono = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        let para = paragraph(lineHeight: 18)
        let dim = NSColor.tertiaryLabelColor, code = NSColor.labelColor
        let tint: NSColor = side == .left ? .systemRed : .systemGreen
        let rowBg = opaque(tint, 0.12), padBg = opaque(.gray, 0.06), hunkBg = opaque(.systemPurple, 0.10)
        let charBg = tint.withAlphaComponent(0.32)
        let out = NSMutableAttributedString()
        var starts: [Int] = [], bg: [NSColor?] = []
        func append(_ s: String, _ color: NSColor) {
            out.append(NSAttributedString(string: s, attributes: [.font: mono, .foregroundColor: color, .paragraphStyle: para]))
        }
        func num(_ n: Int?) -> String {
            let s = n.map(String.init) ?? ""
            return String(repeating: " ", count: max(0, 4 - s.count)) + s
        }
        for r in rows {
            starts.append(out.length)
            if let h = r.hunk {
                bg.append(hunkBg)
                append(side == .left ? h + "\n" : "\n", .systemPurple)
                continue
            }
            let n = side == .left ? r.leftN : r.rightN
            let text = side == .left ? r.left : r.right
            let spans = side == .left ? r.leftSpans : r.rightSpans
            let hi = side == .left ? r.leftHi : r.rightHi
            guard let text else { bg.append(padBg); append("\n", dim); continue }
            bg.append(r.changed ? rowBg : nil)
            append(num(n) + "  ", dim)
            let line = attributedLine(text + "\n", spans: spans, font: mono, base: code, paragraph: para)
            if r.changed, let hi, !hi.isEmpty,
               let l = text.index(text.startIndex, offsetBy: hi.lowerBound, limitedBy: text.endIndex),
               let u = text.index(text.startIndex, offsetBy: hi.upperBound, limitedBy: text.endIndex) {
                line.addAttribute(.backgroundColor, value: charBg, range: NSRange(l..<u, in: text))
            }
            out.append(line)
        }
        return CodeModel(string: out, starts: starts, lineBg: bg)
    }
}

struct CodeSplitView: NSViewRepresentable {
    let file: DiffFile
    var isDark: Bool

    func makeCoordinator() -> Coord { Coord() }

    func makeNSView(context: Context) -> NSView {
        let c = context.coordinator
        let left = CodeTextView(); left.configureReadOnly(inset: NSSize(width: 0, height: 6))
        let right = CodeTextView(); right.configureReadOnly(inset: NSSize(width: 0, height: 6))
        let ls = CodeTextView.makeScroll(left), rs = CodeTextView.makeScroll(right)
        let divider = NSView(); divider.wantsLayer = true
        for v in [ls, rs, divider] { v.translatesAutoresizingMaskIntoConstraints = false }
        let container = NSView()
        container.addSubview(ls); container.addSubview(divider); container.addSubview(rs)
        NSLayoutConstraint.activate([
            ls.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            ls.topAnchor.constraint(equalTo: container.topAnchor),
            ls.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            divider.leadingAnchor.constraint(equalTo: ls.trailingAnchor),
            divider.widthAnchor.constraint(equalToConstant: 1),
            divider.topAnchor.constraint(equalTo: container.topAnchor),
            divider.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            rs.leadingAnchor.constraint(equalTo: divider.trailingAnchor),
            rs.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            rs.topAnchor.constraint(equalTo: container.topAnchor),
            rs.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            ls.widthAnchor.constraint(equalTo: rs.widthAnchor),
        ])
        c.left = left; c.right = right; c.divider = divider
        c.installSync(ls, rs)
        return container
    }

    // Fill the offered space; otherwise the container reports its content-sized subviews'
    // width and the two columns sum widths and overflow instead of splitting the pane.
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSView, context: Context) -> CGSize? {
        proposal.replacingUnspecifiedDimensions(by: CGSize(width: 400, height: 300))
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        nsView.appearance = NSAppearance(named: isDark ? .darkAqua : .aqua)
        let c = context.coordinator
        c.divider?.layer?.backgroundColor = NSColor(Theme.borderSoft).cgColor
        let key = "\(file.path):\(isDark)"
        if c.key == key { return }
        c.key = key
        let rows = splitRows(file)
        c.left?.apply(CodeTextView.splitSide(rows, side: .left))
        c.right?.apply(CodeTextView.splitSide(rows, side: .right))
    }

    final class Coord {
        weak var left: CodeTextView?
        weak var right: CodeTextView?
        weak var divider: NSView?
        var key = ""
        private var syncing = false
        private var tokens: [NSObjectProtocol] = []

        func installSync(_ ls: NSScrollView, _ rs: NSScrollView) {
            for sv in [ls, rs] { sv.contentView.postsBoundsChangedNotifications = true }
            tokens.append(NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: ls.contentView, queue: .main) { [weak self] _ in self?.mirror(from: ls, to: rs) })
            tokens.append(NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: rs.contentView, queue: .main) { [weak self] _ in self?.mirror(from: rs, to: ls) })
        }

        private func mirror(from: NSScrollView, to: NSScrollView) {
            guard !syncing else { return }
            syncing = true; defer { syncing = false }
            let src = from.contentView.bounds.origin
            var o = to.contentView.bounds.origin
            guard abs(o.x - src.x) > 0.5 || abs(o.y - src.y) > 0.5 else { return }
            o.x = src.x; o.y = src.y
            to.contentView.setBoundsOrigin(o)
            to.reflectScrolledClipView(to.contentView)
        }

        deinit { tokens.forEach(NotificationCenter.default.removeObserver) }
    }
}
