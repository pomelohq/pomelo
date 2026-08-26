import SwiftUI
import AppKit

struct YAMLEditor: NSViewRepresentable {
    @Binding var text: String

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    final class QuitAwareTextView: NSTextView {
        override func performKeyEquivalent(with event: NSEvent) -> Bool {
            if event.modifierFlags.contains(.command), !event.modifierFlags.contains(.shift),
               !event.modifierFlags.contains(.option), !event.modifierFlags.contains(.control),
               (event.charactersIgnoringModifiers ?? "").lowercased() == "q" {
                NSApp.terminate(nil); return true
            }
            return super.performKeyEquivalent(with: event)
        }
    }

    func makeNSView(context: Context) -> NSScrollView {
        let tv = QuitAwareTextView()
        tv.isRichText = false
        tv.isAutomaticQuoteSubstitutionEnabled = false
        tv.isAutomaticSpellingCorrectionEnabled = false
        tv.allowsUndo = true
        tv.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        tv.backgroundColor = NSColor(Theme.bg)
        tv.insertionPointColor = NSColor(Theme.accent)
        tv.textContainerInset = NSSize(width: 8, height: 8)
        tv.delegate = context.coordinator
        tv.isHorizontallyResizable = true
        tv.isVerticallyResizable = true
        tv.autoresizingMask = []
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.textContainer?.widthTracksTextView = false
        tv.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.string = text
        context.coordinator.synced = text
        Self.highlight(tv)

        let scroll = NSScrollView()
        scroll.documentView = tv
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = true
        scroll.backgroundColor = NSColor(Theme.bg)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let tv = scroll.documentView as? NSTextView else { return }
        let coord = context.coordinator
        guard text != coord.synced else { return }
        coord.synced = text
        if tv.string != text {
            let sel = tv.selectedRanges
            let origin = scroll.contentView.bounds.origin
            tv.string = text
            Self.highlight(tv)
            let len = (text as NSString).length
            tv.selectedRanges = sel.compactMap { r -> NSValue? in
                let rr = r.rangeValue
                return rr.location <= len ? NSValue(range: NSRange(location: rr.location, length: min(rr.length, len - rr.location))) : nil
            }
            if tv.selectedRanges.isEmpty { tv.selectedRange = NSRange(location: 0, length: 0) }
            scroll.contentView.scroll(to: origin)
            scroll.reflectScrolledClipView(scroll.contentView)
        }
    }

    static func highlight(_ tv: NSTextView) {
        guard let storage = tv.textStorage else { return }
        let s = tv.string as NSString
        let full = NSRange(location: 0, length: s.length)
        storage.beginEditing()
        storage.addAttribute(.foregroundColor, value: NSColor(Theme.fg), range: full)
        storage.addAttribute(.font, value: NSFont.monospacedSystemFont(ofSize: 12, weight: .regular), range: full)
        let key = NSColor(Theme.accent), str = NSColor(Theme.ok), comment = NSColor(Theme.dim), num = NSColor(Theme.warn)
        s.enumerateSubstrings(in: full, options: .byLines) { line, lineRange, _, _ in
            guard let line else { return }
            let t = line as NSString
            let hash = t.range(of: "#")
            if hash.location != NSNotFound {
                storage.addAttribute(.foregroundColor, value: comment,
                                     range: NSRange(location: lineRange.location + hash.location, length: lineRange.length - hash.location))
            }
            let scanEnd = hash.location == NSNotFound ? t.length : hash.location
            if let colon = firstColon(t, upTo: scanEnd) {
                storage.addAttribute(.foregroundColor, value: key,
                                     range: NSRange(location: lineRange.location, length: colon))
            }
            highlightValues(t, base: lineRange.location, end: scanEnd, storage: storage, str: str, num: num)
        }
        // pom {{...}} template refs are the semantic glue — make them pop; flag
        // secret refs distinctly so a missing credential is easy to spot.
        let tmpl = NSColor(Theme.tool), secret = NSColor(Theme.warn)
        if let re = try? NSRegularExpression(pattern: "\\{\\{[^}]*\\}\\}") {
            re.enumerateMatches(in: s as String, range: full) { m, _, _ in
                guard let r = m?.range else { return }
                let isSecret = s.substring(with: r).contains("secret.")
                storage.addAttribute(.foregroundColor, value: isSecret ? secret : tmpl, range: r)
            }
        }
        storage.endEditing()
    }

    private static func firstColon(_ t: NSString, upTo end: Int) -> Int? {
        var i = 0
        while i < end {
            let c = t.character(at: i)
            if c == 0x3A { return i + 1 }
            if c == 0x22 || c == 0x27 { return nil }
            i += 1
        }
        return nil
    }

    private static func highlightValues(_ t: NSString, base: Int, end: Int, storage: NSTextStorage, str: NSColor, num: NSColor) {
        let quote = try? NSRegularExpression(pattern: "\"[^\"]*\"|'[^']*'")
        let nums = try? NSRegularExpression(pattern: "\\b(true|false|null|\\d+(?:\\.\\d+)?)\\b")
        let scan = NSRange(location: 0, length: end)
        quote?.enumerateMatches(in: t as String, range: scan) { m, _, _ in
            if let r = m?.range { storage.addAttribute(.foregroundColor, value: str, range: NSRange(location: base + r.location, length: r.length)) }
        }
        nums?.enumerateMatches(in: t as String, range: scan) { m, _, _ in
            if let r = m?.range { storage.addAttribute(.foregroundColor, value: num, range: NSRange(location: base + r.location, length: r.length)) }
        }
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        let parent: YAMLEditor
        var synced = ""   // text as last pushed either way — distinguishes typing from external loads
        init(_ p: YAMLEditor) { parent = p }
        func textDidChange(_ notification: Notification) {
            guard let tv = notification.object as? NSTextView else { return }
            parent.text = tv.string
            synced = tv.string
            YAMLEditor.highlight(tv)
        }
    }
}
