import Foundation
import CodeEditSourceEditor

// Tree-sitter tokenization for read-only code surfaces (diffs, review), so they tokenize exactly like the built-in
// editor instead of the hand-rolled lexer. Bridges the editor's capture spans into the existing SynSpan/SynKind
// pipeline (SyntaxStyle already resolves SynKind to the editor palette). Returns nil when no grammar is bundled for
// the file's language, so callers can fall back to the lexer.
enum EditorSyntax {
    static func spans(_ line: String, path: String) -> [SynSpan]? {
        guard !line.isEmpty, let caps = SearchExcerptHighlighter.captureSpans(text: line, path: path) else { return nil }
        let ns = line as NSString
        return caps.compactMap { (range, capture) -> SynSpan? in
            guard let kind = synKind(capture) else { return nil }
            let lo = charOffset(ns, range.location)
            let hi = charOffset(ns, min(range.location + range.length, ns.length))
            guard hi > lo else { return nil }
            return SynSpan(lo: lo, hi: hi, kind: kind)
        }
    }

    // UTF-16 offset (tree-sitter/NSString) -> Character offset (SynSpan indexes into Array(line)).
    private static func charOffset(_ ns: NSString, _ utf16: Int) -> Int {
        (ns.substring(to: max(0, min(utf16, ns.length))) as String).count
    }

    // Collapse the editor's fine-grained captures onto SynKind. Plain-colored captures (variables/properties) return
    // nil so the base text color is used, matching the editor.
    private static func synKind(_ c: CaptureName) -> SynKind? {
        switch c {
        case .keyword, .include, .boolean, .conditional, .repeat, .keywordReturn, .keywordFunction, .variableBuiltin:
            return .keyword
        case .string:
            return .string
        case .number, .float, .constant:
            return .number
        case .comment:
            return .comment
        case .type, .constructor, .typeAlternate:
            return .type
        case .function, .method, .tag:
            return .function
        case .attribute:
            return .attribute
        case .variable, .property, .parameter:
            return nil
        }
    }
}
