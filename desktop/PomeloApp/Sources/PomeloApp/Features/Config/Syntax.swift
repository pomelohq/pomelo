import Foundation

enum SynKind: Sendable { case plain, keyword, string, number, comment, type, function, attribute }
struct SynSpan: Sendable { let lo: Int; let hi: Int; let kind: SynKind }

// Detected code language. Drives keyword set + comment/string rules for the lexer.
// Grammar-level (tree-sitter) is only vendored for SQL, so this is a hand-rolled
// lexer good for GitHub-style basic highlighting across the common languages.
enum CodeLang: Sendable {
    case generic, ruby, jsts, java, go, python, swift, clike, rust, php, shell, css, yaml, json

    static func detect(path: String) -> CodeLang {
        let ext = (path as NSString).pathExtension.lowercased()
        switch ext {
        case "rb", "rake", "gemspec", "ru": return .ruby
        case "js", "jsx", "ts", "tsx", "mjs", "cjs": return .jsts
        case "java", "kt", "kts", "scala", "groovy": return .java
        case "go": return .go
        case "py", "pyi", "pyw": return .python
        case "swift": return .swift
        case "c", "h", "cpp", "cc", "hpp", "hh", "cxx", "m", "mm": return .clike
        case "rs": return .rust
        case "php": return .php
        case "sh", "bash", "zsh", "fish": return .shell
        case "css", "scss", "sass", "less": return .css
        case "yml", "yaml": return .yaml
        case "json", "jsonc": return .json
        default:
            let base = (path as NSString).lastPathComponent.lowercased()
            if base == "gemfile" || base == "rakefile" { return .ruby }
            if base == "dockerfile" || base == "makefile" { return .shell }
            return .generic
        }
    }

    var lineComment: [String] {
        switch self {
        case .ruby, .python, .shell, .yaml: return ["#"]
        case .jsts, .java, .go, .swift, .clike, .rust, .php, .css: return ["//"]
        default: return []
        }
    }

    var blockComment: (open: String, close: String)? {
        switch self {
        case .jsts, .java, .go, .swift, .clike, .rust, .php, .css: return ("/*", "*/")
        default: return nil
        }
    }

    var keywords: Set<String> {
        switch self {
        case .ruby: return Keywords.ruby
        case .jsts: return Keywords.jsts
        case .java: return Keywords.java
        case .go: return Keywords.go
        case .python: return Keywords.python
        case .swift: return Keywords.swift
        case .clike: return Keywords.clike
        case .rust: return Keywords.rust
        case .php: return Keywords.php
        case .shell: return Keywords.shell
        default: return Keywords.generic
        }
    }
}

private enum Keywords {
    static let ruby: Set<String> = ["def","end","class","module","if","elsif","else","unless","while","until","return","do","begin","rescue","ensure","then","when","case","yield","self","nil","true","false","and","or","not","in","for","break","next","redo","retry","super","raise","require","require_relative","attr_accessor","attr_reader","attr_writer","include","extend","lambda","proc","new","puts"]
    static let jsts: Set<String> = ["var","let","const","function","return","if","else","for","while","do","switch","case","break","continue","new","class","extends","super","import","export","from","default","async","await","yield","typeof","instanceof","in","of","this","null","undefined","true","false","void","delete","try","catch","finally","throw","interface","type","enum","implements","public","private","protected","readonly","static","get","set","as","namespace","declare"]
    static let java: Set<String> = ["class","interface","enum","extends","implements","public","private","protected","static","final","abstract","void","int","long","double","float","boolean","char","byte","short","if","else","for","while","do","switch","case","break","continue","return","new","this","super","import","package","throws","throw","try","catch","finally","synchronized","volatile","transient","native","instanceof","null","true","false","var","record","sealed"]
    static let go: Set<String> = ["func","package","import","type","struct","interface","map","chan","go","defer","return","if","else","for","range","switch","case","break","continue","fallthrough","default","var","const","nil","true","false","iota","select","goto","make","new","append","len","cap"]
    static let python: Set<String> = ["def","class","return","if","elif","else","for","while","break","continue","import","from","as","pass","lambda","yield","with","try","except","finally","raise","global","nonlocal","in","is","not","and","or","None","True","False","async","await","del","assert","self"]
    static let swift: Set<String> = ["func","let","var","struct","enum","class","protocol","extension","guard","if","else","for","while","switch","case","break","continue","return","import","public","private","internal","fileprivate","open","static","final","lazy","weak","unowned","init","deinit","self","nil","true","false","throws","rethrows","try","catch","throw","defer","as","is","in","where","associatedtype","typealias","some","any","actor","async","await","mutating","override","convenience","required","subscript","didSet","willSet"]
    static let clike: Set<String> = ["int","long","double","float","char","void","short","unsigned","signed","struct","enum","union","typedef","static","const","volatile","extern","register","return","if","else","for","while","do","switch","case","break","continue","goto","sizeof","new","delete","class","public","private","protected","virtual","template","typename","namespace","using","this","nullptr","true","false","auto","inline","operator","friend","explicit"]
    static let rust: Set<String> = ["fn","let","mut","const","struct","enum","trait","impl","pub","use","mod","match","if","else","for","while","loop","break","continue","return","self","Self","as","in","where","move","ref","dyn","async","await","unsafe","crate","super","type","static","true","false","Some","None","Ok","Err","box"]
    static let php: Set<String> = ["function","class","interface","trait","extends","implements","public","private","protected","static","final","abstract","return","if","else","elseif","for","foreach","while","do","switch","case","break","continue","new","echo","print","null","true","false","use","namespace","as","instanceof","try","catch","finally","throw","require","require_once","include","include_once"]
    static let shell: Set<String> = ["if","then","else","elif","fi","for","while","until","do","done","case","esac","function","in","return","export","local","readonly","declare","source","exit","echo"]
    static let generic: Set<String> = ["def","end","class","module","if","else","return","func","let","var","const","function","import","export","from","new","async","await","for","while","switch","struct","enum","public","private","static","void","package","type","interface","try","catch","throw","true","false","nil","null"]
}

enum Syntax {
    // Kept for the diff views: a single generic line, no language context.
    static let keywords = Keywords.generic
    static func spans(_ s: String) -> [SynSpan] { lineSpans(Array(s), lang: .generic, keywords: Keywords.generic) }

    // Language-aware, whole-file tokenizer. Returns spans per line and carries
    // block-comment / triple-string state across line boundaries so a `/* ... */`
    // spanning many lines greys the whole thing (a per-line pass cannot).
    static func tokenize(_ content: String, lang: CodeLang) -> [[SynSpan]] {
        let lines = content.components(separatedBy: "\n")
        let kw = lang.keywords
        let block = lang.blockComment
        var out: [[SynSpan]] = []
        out.reserveCapacity(lines.count)
        var carry: String? = nil // closing token of an open block comment / triple string
        for line in lines {
            let c = Array(line)
            var spans: [SynSpan] = []
            var i = 0
            if let close = carry {
                if let r = findClose(c, close, from: 0) {
                    spans.append(SynSpan(lo: 0, hi: r, kind: .comment)); i = r; carry = nil
                } else {
                    spans.append(SynSpan(lo: 0, hi: c.count, kind: .comment)); out.append(spans); continue
                }
            }
            let (ls, ncarry) = lineSpansStateful(c, from: i, lang: lang, keywords: kw, block: block)
            spans.append(contentsOf: ls)
            carry = ncarry
            out.append(spans)
        }
        return out
    }

    private static func findClose(_ c: [Character], _ token: String, from: Int) -> Int? {
        let t = Array(token), n = c.count, m = t.count
        guard m > 0 else { return from }
        var i = from
        while i + m <= n {
            if Array(c[i..<i+m]) == t { return i + m }
            i += 1
        }
        return nil
    }

    private static func ident(_ ch: Character) -> Bool { ch.isLetter || ch.isNumber || ch == "_" }

    // Non-stateful path used by the generic diff highlighter.
    private static func lineSpans(_ c: [Character], lang: CodeLang, keywords: Set<String>) -> [SynSpan] {
        lineSpansStateful(c, from: 0, lang: lang, keywords: keywords, block: nil).0
    }

    private static func lineSpansStateful(_ c: [Character], from: Int, lang: CodeLang, keywords: Set<String>,
                                          block: (open: String, close: String)?) -> ([SynSpan], String?) {
        var out: [SynSpan] = []
        let n = c.count
        var i = from
        let lineTokens = lang == .generic ? ["#", "//"] : lang.lineComment
        func matches(_ tok: String, at p: Int) -> Bool {
            let t = Array(tok); guard p + t.count <= n else { return false }
            return Array(c[p..<p+t.count]) == t
        }
        while i < n {
            let ch = c[i]
            if lineTokens.contains(where: { matches($0, at: i) }) {
                out.append(SynSpan(lo: i, hi: n, kind: .comment)); break
            }
            if let block, matches(block.open, at: i) {
                if let end = findClose(c, block.close, from: i + block.open.count) {
                    out.append(SynSpan(lo: i, hi: end, kind: .comment)); i = end; continue
                }
                out.append(SynSpan(lo: i, hi: n, kind: .comment)); return (out, block.close)
            }
            if ch == "\"" || ch == "'" || ch == "`" {
                var j = i + 1
                while j < n && c[j] != ch { if c[j] == "\\" { j += 1 }; j += 1 }
                let end = min(j + 1, n)
                out.append(SynSpan(lo: i, hi: end, kind: .string)); i = end; continue
            }
            if ch.isNumber {
                var j = i + 1
                while j < n && (c[j].isNumber || c[j] == "." || c[j] == "x" || c[j] == "_"
                               || ("a"..."f").contains(c[j]) || ("A"..."F").contains(c[j])) { j += 1 }
                out.append(SynSpan(lo: i, hi: j, kind: .number)); i = j; continue
            }
            if ch.isLetter || ch == "_" || ch == "$" || ch == "@" {
                var j = i + 1
                while j < n && (ident(c[j]) || c[j] == "$") { j += 1 }
                let word = String(c[i..<j])
                if keywords.contains(word) {
                    out.append(SynSpan(lo: i, hi: j, kind: .keyword))
                } else {
                    var k = j
                    while k < n && c[k] == " " { k += 1 }
                    if k < n && c[k] == "(" {
                        out.append(SynSpan(lo: i, hi: j, kind: .function))
                    } else if let f = ch.unicodeScalars.first, f.properties.isUppercase {
                        out.append(SynSpan(lo: i, hi: j, kind: .type))
                    }
                }
                i = j; continue
            }
            i += 1
        }
        return (out, nil)
    }
}
