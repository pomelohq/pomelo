import SwiftUI
// Theme, ThemeMode, Palette, ThemeManager now live in the shared PomeloUI package;
// re-export so existing `Theme.*` call sites need no per-file import.
@_exported import PomeloUI

struct SectionLabel: View {
    let text: String
    var size: CGFloat = 10.5
    var body: some View {
        Text(text.uppercased()).font(.system(size: size, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
    }
}

struct StatusPill: View {
    let text: String
    var color: Color
    var uppercase: Bool = false
    var body: some View {
        Text(uppercase ? text.uppercased() : text)
            .font(uppercase ? Theme.mono(9.5, .semibold) : .system(size: 10, weight: .semibold))
            .kerning(uppercase ? 0.5 : 0)
            .foregroundStyle(color)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(color.opacity(0.15), in: Capsule())
    }
}

struct Card<Content: View>: View {
    var cornerRadius: CGFloat = 8
    var background: Color = Theme.surface
    @ViewBuilder var content: () -> Content
    var body: some View {
        content()
            .background(background)
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
            .overlay(RoundedRectangle(cornerRadius: cornerRadius).strokeBorder(Theme.borderSoft))
    }
}
