import SwiftUI
import AppKit

// Shared read-only code viewer: one NSTextView-backed component (GPU-backed scrolling,
// syntax highlight, gutter, a tinted line range) reused by every code surface instead
// of each screen rolling its own. Mirrors CodeDiffView's approach.
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
        return CodeTextViewBase.makeScroll(tv)
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let ap = NSAppearance(named: isDark ? .darkAqua : .aqua)
        scroll.appearance = ap
        guard let tv = context.coordinator.textView else { return }
        tv.appearance = ap
        context.coordinator.onSelectLines = onSelectLines
        let key = "\(content.count):\(start):\(end):\(isDark)"
        if context.coordinator.key == key { return }
        context.coordinator.key = key
        let built = CodeTextView.build(content, lang: lang, start: start, end: end)
        tv.hitLo = built.hitLo
        tv.hitHi = built.hitHi
        tv.lineStarts = built.starts
        tv.textStorage?.setAttributedString(built.string)
        if let tc = tv.textContainer { tv.layoutManager?.ensureLayout(for: tc) }
        tv.needsDisplay = true
        DispatchQueue.main.async { scrollToTarget(tv) }
    }

    // AppKit handles clip-view/frame timing; a manual boundingRect scroll landed at
    // top before layout settled. Double-scroll pins the anchor near the top with context.
    private func scrollToTarget(_ tv: CodeTextView) {
        guard start > 1, start - 1 < tv.lineStarts.count else { return }
        let below = min(tv.hitHi + 10, tv.lineStarts.count - 1)
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

// Shared parent for the read-only code text views (CodeView + CodeDiffView): line
// index bookkeeping, the NSTextView read-only config, the scroll host, one line-height
// paragraph builder, and the single syntax-highlighted-line builder.
class CodeTextViewBase: NSTextView {
    var lineStarts: [Int] = []

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

    // One syntax-highlighted code line — the shared core; each subclass supplies its
    // own font/base/paragraph and its own gutter/backdrop.
    static func attributedLine(_ text: String, spans: [SynSpan], font: NSFont, base: NSColor,
                               paragraph: NSParagraphStyle? = nil) -> NSMutableAttributedString {
        var attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: base]
        if let paragraph { attrs[.paragraphStyle] = paragraph }
        let a = NSMutableAttributedString(string: text, attributes: attrs)
        for sp in spans where sp.kind != .plain {
            guard let l = text.index(text.startIndex, offsetBy: sp.lo, limitedBy: text.endIndex),
                  let h = text.index(text.startIndex, offsetBy: sp.hi, limitedBy: text.endIndex), l < h else { continue }
            a.addAttribute(.foregroundColor, value: SyntaxStyle.nsColor(sp.kind), range: NSRange(l..<h, in: text))
        }
        return a
    }
}

final class CodeTextView: CodeTextViewBase {
    var hitLo = -1   // 0-based first highlighted line (inclusive)
    var hitHi = -1   // 0-based last highlighted line (inclusive)

    private static let mono = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
    private static let para = CodeTextViewBase.paragraph(lineHeight: 20)

    static func build(_ content: String, lang: CodeLang, start: Int, end: Int)
        -> (string: NSAttributedString, starts: [Int], hitLo: Int, hitHi: Int) {
        let lines = content.components(separatedBy: "\n")
        let spansPerLine = Syntax.tokenize(content, lang: lang)
        let gutterW = max(3, String(lines.count).count)
        let dim = NSColor.tertiaryLabelColor, code = NSColor.labelColor
        let out = NSMutableAttributedString()
        var starts: [Int] = []
        let hitLo = start > 0 ? min(start, lines.count) - 1 : -1
        let hitHi = start > 0 ? min(max(start, end), lines.count) - 1 : -1
        for (i, line) in lines.enumerated() {
            starts.append(out.length)
            let num = String(i + 1)
            let pad = String(repeating: " ", count: max(0, gutterW - num.count))
            out.append(NSAttributedString(string: pad + num + "  ", attributes: [.font: mono, .foregroundColor: dim, .paragraphStyle: para]))
            let spans = i < spansPerLine.count ? spansPerLine[i] : []
            out.append(attributedLine(line + "\n", spans: spans, font: mono, base: code, paragraph: para))
        }
        return (out, starts, hitLo, hitHi)
    }

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard hitLo >= 0, hitLo < lineStarts.count, let lm = layoutManager, let tc = textContainer,
              let storage = textStorage else { return }
        let firstChar = lineStarts[hitLo]
        let lastChar = hitHi + 1 < lineStarts.count ? lineStarts[hitHi + 1] : storage.length
        let glyphRange = lm.glyphRange(forCharacterRange: NSRange(location: firstChar, length: max(0, lastChar - firstChar)),
                                       actualCharacterRange: nil)
        var r = lm.boundingRect(forGlyphRange: glyphRange, in: tc)
        r.origin.y += textContainerInset.height
        r.origin.x = 0
        r.size.width = max(bounds.width, enclosingScrollView?.contentSize.width ?? 0)
        NSColor.controlAccentColor.withAlphaComponent(0.16).setFill()
        r.fill()
    }
}
