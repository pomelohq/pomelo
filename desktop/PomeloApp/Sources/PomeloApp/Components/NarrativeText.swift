import SwiftUI
import AppKit

struct NarrativeText: NSViewRepresentable {
    let text: String
    var isDark: Bool
    var maxWidth: CGFloat = 760
    var onLink: (URL) -> Void = { _ in }

    func makeCoordinator() -> Coord { Coord(onLink: onLink) }

    func makeNSView(context: Context) -> NSTextView {
        let tv = NSTextView()
        tv.isEditable = false
        tv.isSelectable = true
        tv.drawsBackground = false
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = false
        tv.textContainerInset = .zero
        tv.textContainer?.lineFragmentPadding = 0
        tv.textContainer?.widthTracksTextView = true
        tv.delegate = context.coordinator
        tv.selectedTextAttributes = [.backgroundColor: NSColor(Theme.sel)]
        tv.linkTextAttributes = [.foregroundColor: NSColor(Theme.accent), .cursor: NSCursor.pointingHand]
        return tv
    }

    func updateNSView(_ tv: NSTextView, context: Context) {
        tv.appearance = NSAppearance(named: isDark ? .darkAqua : .aqua)
        context.coordinator.onLink = onLink
        let key = "\(text.count):\(isDark)"
        guard context.coordinator.key != key else { return }
        context.coordinator.key = key
        tv.textStorage?.setAttributedString(NarrativeBuilder.build(text))
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView tv: NSTextView, context: Context) -> CGSize? {
        let w = min(proposal.width ?? maxWidth, maxWidth)
        guard let tc = tv.textContainer, let lm = tv.layoutManager else { return nil }
        tc.containerSize = NSSize(width: w, height: .greatestFiniteMagnitude)
        lm.ensureLayout(for: tc)
        return CGSize(width: w, height: ceil(lm.usedRect(for: tc).height))
    }

    final class Coord: NSObject, NSTextViewDelegate {
        var onLink: (URL) -> Void
        var key = ""
        init(onLink: @escaping (URL) -> Void) { self.onLink = onLink }
        func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
            let url = (link as? URL) ?? (link as? String).flatMap { URL(string: $0) }
            if let url { onLink(url); return true }
            return false
        }
    }
}

private enum NarrativeBuilder {
    static func build(_ md: String) -> NSAttributedString {
        let out = NSMutableAttributedString()
        let body = NSFont.systemFont(ofSize: 14)
        let fg = NSColor(Theme.fgSoft)
        let lines = md.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        var i = 0

        func para(_ s: String, font: NSFont, color: NSColor, before: CGFloat, after: CGFloat, indent: CGFloat = 0, marker: String? = nil, justify: Bool = false) {
            let p = NSMutableParagraphStyle()
            p.lineSpacing = 5
            p.alignment = justify ? .justified : .natural
            p.paragraphSpacingBefore = before
            p.paragraphSpacing = after
            p.headIndent = indent
            p.firstLineHeadIndent = marker == nil ? indent : max(0, indent - 18)
            let line = NSMutableAttributedString()
            if let marker {
                line.append(NSAttributedString(string: marker, attributes: [.font: font, .foregroundColor: NSColor(Theme.dim)]))
            }
            line.append(inline(s, base: font, color: color))
            line.append(NSAttributedString(string: "\n"))
            line.addAttribute(.paragraphStyle, value: p, range: NSRange(location: 0, length: line.length))
            out.append(line)
        }

        func headingFont(_ n: Int) -> NSFont {
            NSFont.systemFont(ofSize: n <= 1 ? 20 : n == 2 ? 16.5 : 14.5, weight: .semibold)
        }

        while i < lines.count {
            let raw = lines[i]
            let t = raw.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") {
                i += 1
                var code: [String] = []
                while i < lines.count, !lines[i].trimmingCharacters(in: .whitespaces).hasPrefix("```") { code.append(lines[i]); i += 1 }
                i += 1
                let p = NSMutableParagraphStyle(); p.paragraphSpacingBefore = 6; p.paragraphSpacing = 8; p.lineSpacing = 2
                let s = NSMutableAttributedString(string: code.joined(separator: "\n") + "\n",
                    attributes: [.font: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular),
                                 .foregroundColor: NSColor(Theme.fg), .paragraphStyle: p])
                out.append(s)
                continue
            }
            if t == "---" || t == "***" || t == "___" {
                let p = NSMutableParagraphStyle(); p.paragraphSpacingBefore = 6; p.paragraphSpacing = 6
                out.append(NSAttributedString(string: "\u{2500}\u{2500}\u{2500}\u{2500}\n",
                    attributes: [.font: body, .foregroundColor: NSColor(Theme.borderSoft), .paragraphStyle: p]))
                i += 1; continue
            }
            if let h = heading(t) { para(h.1, font: headingFont(h.0), color: NSColor(Theme.fg), before: 10, after: 4); i += 1; continue }
            if t.hasPrefix("> ") || t == ">" {
                var q: [String] = []
                while i < lines.count, lines[i].trimmingCharacters(in: .whitespaces).hasPrefix(">") {
                    q.append(String(lines[i].trimmingCharacters(in: .whitespaces).dropFirst()).trimmingCharacters(in: .whitespaces)); i += 1
                }
                para(q.joined(separator: " "), font: italic(body), color: NSColor(Theme.fgMuted), before: 4, after: 8, indent: 14)
                continue
            }
            if listItem(t) != nil {
                var n = 0
                while i < lines.count, let item = listItem(lines[i].trimmingCharacters(in: .whitespaces)) {
                    let depth = leadingSpaces(lines[i]) / 2
                    let ordered = orderedItem(lines[i].trimmingCharacters(in: .whitespaces)) != nil
                    n += 1
                    let marker = ordered ? "\(n).  " : "\u{2022}  "
                    para(item, font: body, color: fg, before: 0, after: 3, indent: CGFloat(18 + depth * 16), marker: marker)
                    i += 1
                }
                continue
            }
            if t.isEmpty { i += 1; continue }
            var buf: [String] = []
            while i < lines.count {
                let lt = lines[i].trimmingCharacters(in: .whitespaces)
                if lt.isEmpty || isBlockStart(lt) { break }
                buf.append(lines[i]); i += 1
            }
            para(buf.joined(separator: " "), font: body, color: fg, before: 0, after: 9, justify: true)
        }
        return out
    }

    private static func inline(_ s: String, base: NSFont, color: NSColor) -> NSAttributedString {
        guard let a = try? AttributedString(markdown: s, options: .init(
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible)) else {
            return NSAttributedString(string: s, attributes: [.font: base, .foregroundColor: color])
        }
        let ns = NSMutableAttributedString()
        for run in a.runs {
            let piece = String(a[run.range].characters)
            var font = base
            var col = color
            var bg: NSColor?
            let intent = run.inlinePresentationIntent ?? []
            if intent.contains(.stronglyEmphasized) { font = bold(font) }
            if intent.contains(.emphasized) { font = italic(font) }
            if intent.contains(.code) {
                font = NSFont.monospacedSystemFont(ofSize: base.pointSize - 2, weight: .regular)
                bg = NSColor(Theme.chip); col = NSColor(Theme.fg)
            }
            var attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: col]
            if let bg { attrs[.backgroundColor] = bg }
            if let link = run.link { attrs[.link] = link }
            ns.append(NSAttributedString(string: piece, attributes: attrs))
        }
        return ns
    }

    private static func bold(_ f: NSFont) -> NSFont { NSFontManager.shared.convert(f, toHaveTrait: .boldFontMask) }
    private static func italic(_ f: NSFont) -> NSFont { NSFontManager.shared.convert(f, toHaveTrait: .italicFontMask) }

    private static func heading(_ t: String) -> (Int, String)? {
        guard t.hasPrefix("#") else { return nil }
        var n = 0
        for c in t { if c == "#" { n += 1 } else { break } }
        guard n <= 6, t.count > n, t[t.index(t.startIndex, offsetBy: n)] == " " else { return nil }
        return (n, String(t.dropFirst(n + 1)))
    }
    private static func orderedItem(_ t: String) -> String? {
        let parts = t.prefix(while: { $0 != " " })
        if let dot = parts.last, dot == ".", parts.dropLast().allSatisfy(\.isNumber), !parts.isEmpty {
            return String(t.dropFirst(parts.count + 1))
        }
        return nil
    }
    private static func listItem(_ t: String) -> String? {
        if t.hasPrefix("- ") || t.hasPrefix("* ") || t.hasPrefix("+ ") { return String(t.dropFirst(2)) }
        return orderedItem(t)
    }
    private static func leadingSpaces(_ s: String) -> Int {
        var n = 0
        for c in s { if c == " " { n += 1 } else if c == "\t" { n += 4 } else { break } }
        return n
    }
    private static func isBlockStart(_ t: String) -> Bool {
        t.hasPrefix("#") || t.hasPrefix("```") || t.hasPrefix("> ") || listItem(t) != nil
            || t == "---" || t == "***" || t == "___"
    }
}
