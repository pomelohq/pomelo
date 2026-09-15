import AppKit
import CodeEditSourceEditor

// App-side convenience over the editor package's tree-sitter highlighter: colored `AttributedString` lines for a
// project-search excerpt block, or nil if the file has no bundled grammar.
enum SearchSyntax {
    static func highlight(blockText: String, path: String, theme: EditorTheme) -> [AttributedString]? {
        SearchExcerptHighlighter.highlight(text: blockText, path: path, theme: theme)?.map { AttributedString($0) }
    }
}
