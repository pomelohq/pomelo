import SwiftUI
import AppKit

// One palette + one highlighter for every read-only code surface (review peek, flow
// timeline, diffs) so syntax colours are identical everywhere instead of each view
// picking its own.
enum SyntaxStyle {
    static func color(_ k: SynKind) -> Color {
        switch k {
        case .keyword:  return Theme.accent
        case .string:   return Theme.ok
        case .number:   return Theme.warn
        case .comment:  return Theme.dim
        case .type:     return Theme.tool
        case .function: return Theme.wsAccent
        case .plain:    return Theme.fgSoft
        }
    }

    static func nsColor(_ k: SynKind) -> NSColor {
        switch k {
        case .keyword:  return NSColor(Theme.accent)
        case .string:   return NSColor(Theme.ok)
        case .number:   return NSColor(Theme.warn)
        case .comment:  return NSColor(Theme.dim)
        case .type:     return NSColor(Theme.tool)
        case .function: return NSColor(Theme.wsAccent)
        case .plain:    return NSColor(Theme.fgSoft)
        }
    }

    // A single highlighted line as SwiftUI Text (used by row-based renderers).
    static func text(_ line: String, spans: [SynSpan], size: CGFloat) -> Text {
        guard !line.isEmpty else { return Text(" ").font(Theme.mono(size)) }
        var a = AttributedString(line)
        a.foregroundColor = Theme.fgSoft
        let n = line.count
        func idx(_ o: Int) -> AttributedString.Index { a.characters.index(a.startIndex, offsetBy: min(max(o, 0), n)) }
        for sp in spans where sp.kind != .plain {
            a[idx(sp.lo)..<idx(sp.hi)].foregroundColor = color(sp.kind)
        }
        return Text(a).font(Theme.mono(size))
    }
}
