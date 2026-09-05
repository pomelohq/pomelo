import SwiftUI

// Portable design-system primitives shared across platforms. Deliberately NO native
// controls (ProgressView / Picker / Toggle / Stepper) — everything is drawn from SwiftUI
// shapes so the visual language stays ours and portable.

// Custom indeterminate spinner (replaces ProgressView everywhere).
public struct Spinner: View {
    var size: CGFloat
    var lineWidth: CGFloat
    var color: Color
    @State private var spin = false
    public init(size: CGFloat = 12, lineWidth: CGFloat = 1.6, color: Color = Theme.accent) {
        self.size = size; self.lineWidth = lineWidth; self.color = color
    }
    public var body: some View {
        Circle().trim(from: 0, to: 0.7)
            .stroke(color, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
            .frame(width: size, height: size)
            .rotationEffect(.degrees(spin ? 360 : 0))
            .onAppear { withAnimation(.linear(duration: 0.75).repeatForever(autoreverses: false)) { spin = true } }
    }
}

// The ubiquitous plain icon button with a hover background (nav, toolbar, refresh…).
public struct IconButton: View {
    let systemName: String
    var size: CGFloat
    var tip: String?
    let action: () -> Void
    @State private var hover = false
    // Depend on the appearance so a theme switch re-evaluates the body and re-tints.
    @Environment(\.colorScheme) private var colorScheme

    public init(_ systemName: String, size: CGFloat = 12, tip: String? = nil, action: @escaping () -> Void) {
        self.systemName = systemName; self.size = size; self.tip = tip; self.action = action
    }

    public var body: some View {
        let _ = colorScheme
        return Button(action: action) {
            Image(systemName: systemName).font(.system(size: size))
                .foregroundStyle(hover ? Theme.fg : Theme.dim)
                .frame(width: 24, height: 22)
                .background(hover ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 5))
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .modifier(OptionalHelp(tip))
    }
}

struct OptionalHelp: ViewModifier {
    let text: String?
    init(_ text: String?) { self.text = text }
    func body(content: Content) -> some View {
        if let t = text { content.help(t) } else { content }
    }
}

// Surface + soft border + rounded corners — the standard panel/card chrome.
public struct Card<Content: View>: View {
    var cornerRadius: CGFloat
    var background: Color
    @ViewBuilder var content: () -> Content
    public init(cornerRadius: CGFloat = 8, background: Color = Theme.surface, @ViewBuilder content: @escaping () -> Content) {
        self.cornerRadius = cornerRadius; self.background = background; self.content = content
    }
    public var body: some View {
        content()
            .background(background)
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
            .overlay(RoundedRectangle(cornerRadius: cornerRadius).strokeBorder(Theme.borderSoft))
    }
}

// Tinted capsule label for a status/state (open/merged/approved/local/jira status…).
public struct StatusPill: View {
    let text: String
    var color: Color
    var uppercase: Bool
    public init(text: String, color: Color, uppercase: Bool = false) {
        self.text = text; self.color = color; self.uppercase = uppercase
    }
    public var body: some View {
        Text(uppercase ? text.uppercased() : text)
            .font(uppercase ? Theme.mono(9.5, .semibold) : .system(size: 10, weight: .semibold))
            .kerning(uppercase ? 0.5 : 0)
            .foregroundStyle(color)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(color.opacity(0.15), in: Capsule())
    }
}

// A row that highlights when active and fires onSelect on tap.
public struct SelectableRow<Content: View>: View {
    let isActive: Bool
    var cornerRadius: CGFloat
    var activeColor: Color
    let onSelect: () -> Void
    @ViewBuilder var content: () -> Content
    public init(isActive: Bool, cornerRadius: CGFloat = 7, activeColor: Color = Theme.sel,
                onSelect: @escaping () -> Void, @ViewBuilder content: @escaping () -> Content) {
        self.isActive = isActive; self.cornerRadius = cornerRadius; self.activeColor = activeColor
        self.onSelect = onSelect; self.content = content
    }
    public var body: some View {
        content()
            .background(isActive ? activeColor : .clear, in: RoundedRectangle(cornerRadius: cornerRadius))
            .contentShape(Rectangle())
            .onTapGesture(perform: onSelect)
    }
}

// Custom (non-native) segmented tab bar — replaces button-tab clusters and Picker.
public struct SegmentedTabs<Tab: Hashable>: View {
    let tabs: [Tab]
    @Binding var selection: Tab
    var label: (Tab) -> String
    var accent: Bool
    public init(tabs: [Tab], selection: Binding<Tab>, label: @escaping (Tab) -> String, accent: Bool = true) {
        self.tabs = tabs; self._selection = selection; self.label = label; self.accent = accent
    }
    public var body: some View {
        HStack(spacing: 2) {
            ForEach(tabs, id: \.self) { t in
                Button { selection = t } label: {
                    Text(label(t)).font(.system(size: 12, weight: selection == t ? .semibold : .regular))
                        .foregroundStyle(selection == t ? (accent ? Theme.accent : Theme.fg) : Theme.fgMuted)
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(selection == t ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 6))
                        .contentShape(Rectangle())
                }.buttonStyle(.plain)
            }
        }
    }
}

/// A "?" badge that explains a setting on hover. Deliberately inert on click.
public struct HelpHint: View {
    let text: String
    public init(_ text: String) { self.text = text }
    public var body: some View {
        Image(systemName: "questionmark.circle.fill")
            .font(.system(size: 11.5))
            .foregroundStyle(Theme.dim)
            .contentShape(Circle())
            .help(text)
            .accessibilityLabel(text)
    }
}
