import AppKit
import Foundation
import SwiftTreeSitter
import CodeEditLanguages

/// Standalone tree-sitter syntax highlighting for short code excerpts (e.g. project-search results), reusing the
/// editor's grammar + theme so results read like real code — the way Zed's search multibuffer does. Returns one
/// colored line per line of `text`, or nil when the file has no bundled grammar.
public enum SearchExcerptHighlighter {
    public static func highlight(text: String, path: String, theme: EditorTheme) -> [NSAttributedString]? {
        let lang = CodeLanguage.detectLanguageFrom(url: URL(fileURLWithPath: path))
        guard let tsLanguage = lang.language,
              let query = TreeSitterModel.shared.query(for: lang.id) else { return nil }

        let parser = Parser()
        do { try parser.setLanguage(tsLanguage) } catch { return nil }
        guard let tree = parser.parse(text), let root = tree.rootNode else { return nil }

        let full = NSMutableAttributedString(string: text)
        full.addAttribute(.foregroundColor, value: theme.text.color,
                          range: NSRange(location: 0, length: full.length))

        let cursor = query.execute(node: root, in: tree)
        for match in cursor {
            for capture in match.captures {
                let color = theme.colorFor(CaptureName.fromString(capture.name))
                let r = capture.range
                guard r.location >= 0, r.location + r.length <= full.length else { continue }
                full.addAttribute(.foregroundColor, value: color, range: r)
            }
        }

        // Split on "\n" deterministically so the count always equals (newlines + 1), matching the caller's
        // line array even when the excerpt ends in a blank line (NSString.lineRange would drop that final empty line).
        let ns = full.string as NSString
        var out: [NSAttributedString] = []
        var start = 0
        for i in 0..<ns.length {
            if ns.character(at: i) == 0x0A {
                out.append(full.attributedSubstring(from: NSRange(location: start, length: i - start)))
                start = i + 1
            }
        }
        out.append(full.attributedSubstring(from: NSRange(location: start, length: ns.length - start)))
        return out
    }
}
