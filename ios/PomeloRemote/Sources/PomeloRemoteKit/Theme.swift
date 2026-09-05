import SwiftUI
// Theme, ThemeMode, Palette, ThemeManager now live in the shared PomeloUI package;
// re-export so existing `Theme.*` call sites need no per-file import.
@_exported import PomeloUI

// StatusPill and Card now come from the shared PomeloUI package (re-exported above).
// SectionLabel stays app-local: the iOS variant uppercases its input.
struct SectionLabel: View {
    let text: String
    var size: CGFloat = 10.5
    var body: some View {
        Text(text.uppercased()).font(.system(size: size, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
    }
}
