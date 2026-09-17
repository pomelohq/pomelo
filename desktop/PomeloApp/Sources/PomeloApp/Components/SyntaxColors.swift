import SwiftUI
import AppKit
import CodeEditSourceEditor

// One palette + one highlighter for every read-only code surface (review peek, flow
// timeline, diffs). Colours come straight from the built-in editor theme
// (SQLEditor.palette, theme-aware) so read-only code reads identically to the editor.
enum SyntaxStyle {
    static func color(_ k: SynKind) -> Color { Color(nsColor: nsColor(k)) }

    static func nsColor(_ k: SynKind) -> NSColor {
        let t = SQLEditor.palette(activeThemeMode)
        switch k {
        case .keyword:  return t.keywords.color
        case .string:   return t.strings.color
        case .number:   return t.numbers.color
        case .comment:  return t.comments.color
        case .type:     return t.types.color
        case .function: return t.commands.color
        case .attribute: return t.attributes.color
        case .plain:    return t.text.color
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
