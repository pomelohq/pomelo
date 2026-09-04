import SwiftUI
// Theme, ThemeMode, Palette, ThemeManager now live in the shared PomeloUI package;
// re-export so existing `Theme.*` / `ThemeManager` call sites need no per-file import.
@_exported import PomeloUI

/// How read-only code views handle a line wider than the pane. A preference
/// because it is a genuine split: wrapping keeps everything on screen, scrolling
/// keeps the split diff's two columns aligned line-for-line.
enum CodeWrapMode: String, CaseIterable {
    case wrap, scroll
    var wraps: Bool { self == .wrap }
    var label: String { self == .wrap ? "Wrap" : "Scroll" }
}

// Read by AppKit render code, which has no view context to inject through.
var activeCodeWrapMode: CodeWrapMode =
    CodeWrapMode(rawValue: UserDefaults.standard.string(forKey: "codeWrapMode") ?? "scroll") ?? .scroll

final class CodeDisplayManager: ObservableObject {
    static let shared = CodeDisplayManager()
    @Published var wrapMode: CodeWrapMode = activeCodeWrapMode {
        didSet {
            activeCodeWrapMode = wrapMode
            UserDefaults.standard.set(wrapMode.rawValue, forKey: "codeWrapMode")
        }
    }
    @Published var defaultSplit: Bool = UserDefaults.standard.bool(forKey: "diffDefaultSplit") {
        didSet { UserDefaults.standard.set(defaultSplit, forKey: "diffDefaultSplit") }
    }
}
