import AppKit
import CodeEditSourceEditor

// App-side convenience over the editor package's tree-sitter highlighter: colored `AttributedString` lines for a
// project-search excerpt block, or nil if the file has no bundled grammar.
enum SearchSyntax {
    static func highlightNS(blockText: String, path: String, theme: EditorTheme) -> [NSAttributedString]? {
        SearchExcerptHighlighter.highlight(text: blockText, path: path, theme: theme)
    }
}
