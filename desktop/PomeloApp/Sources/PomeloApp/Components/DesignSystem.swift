import SwiftUI

// Portable primitives for our design system. Deliberately NO native macOS controls
// (ProgressView / Picker / Toggle / Stepper / .controlSize) — everything is drawn
// from SwiftUI shapes so the visual language is ours and stays portable off Apple.

// Custom indeterminate spinner (replaces ProgressView everywhere).
struct Spinner: View {
    var size: CGFloat = 12
    var lineWidth: CGFloat = 1.6
    var color: Color = Theme.accent
    @State private var spin = false
    var body: some View {
        Circle().trim(from: 0, to: 0.7)
            .stroke(color, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
            .frame(width: size, height: size)
            .rotationEffect(.degrees(spin ? 360 : 0))
            .onAppear { withAnimation(.linear(duration: 0.75).repeatForever(autoreverses: false)) { spin = true } }
    }
}

// The ubiquitous plain icon button with a hover background (nav, toolbar, refresh…).
struct IconButton: View {
    let systemName: String
    var size: CGFloat = 12
    var tip: String? = nil
    let action: () -> Void
    @State private var hover = false
    // Reads the static Theme colors; depend on the appearance so a theme switch
    // (which flips NSApp.appearance) re-evaluates the body and re-tints the icon.
    @Environment(\.colorScheme) private var colorScheme

    init(_ systemName: String, size: CGFloat = 12, tip: String? = nil, action: @escaping () -> Void) {
        self.systemName = systemName; self.size = size; self.tip = tip; self.action = action
    }

    var body: some View {
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

private struct OptionalHelp: ViewModifier {
    let text: String?
    init(_ text: String?) { self.text = text }
    func body(content: Content) -> some View {
        if let t = text { content.help(t) } else { content }
    }
}

// Surface + soft border + rounded corners — the standard panel/card chrome.
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

// Uppercase kerned section label used across headers.
struct SectionLabel: View {
    // Reads Theme.* statically; observe the theme so a switch invalidates this view
    // even though `text` is unchanged (else SwiftUI reuses the stale body + old palette).
    @EnvironmentObject var theme: ThemeManager
    let text: String
    var size: CGFloat = 10.5
    var body: some View {
        Text(text).font(.system(size: size, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
    }
}

// Collapsible section header: chevron + label + optional count + optional trailing.
struct SectionHeader<Trailing: View>: View {
    @EnvironmentObject var theme: ThemeManager
    let title: String
    @Binding var expanded: Bool
    var count: Int? = nil
    var loading: Bool = false
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        HStack(spacing: 8) {
            Button { withAnimation(.easeInOut(duration: 0.14)) { expanded.toggle() } } label: {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(Theme.muted).rotationEffect(.degrees(expanded ? 90 : 0))
                    SectionLabel(text: title)
                    if let c = count {
                        Text("\(c)").font(Theme.mono(9.5)).foregroundStyle(Theme.fgMuted)
                            .padding(.horizontal, 5).padding(.vertical, 1).background(Theme.dim.opacity(0.15), in: Capsule())
                    }
                    if loading { Spinner(size: 10) }
                }
                .contentShape(Rectangle())
            }.buttonStyle(.plain)
            Spacer()
            trailing()
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(Theme.bgSoft)
    }
}

extension SectionHeader where Trailing == EmptyView {
    init(title: String, expanded: Binding<Bool>, count: Int? = nil, loading: Bool = false) {
        self.init(title: title, expanded: expanded, count: count, loading: loading, trailing: { EmptyView() })
    }
}

// Tinted capsule label for a status/state (open/merged/approved/local/jira status…).
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

// A row that highlights when active and fires onSelect on tap.
struct SelectableRow<Content: View>: View {
    let isActive: Bool
    var cornerRadius: CGFloat = 7
    var activeColor: Color = Theme.sel
    let onSelect: () -> Void
    @ViewBuilder var content: () -> Content
    var body: some View {
        content()
            .background(isActive ? activeColor : .clear, in: RoundedRectangle(cornerRadius: cornerRadius))
            .contentShape(Rectangle())
            .onTapGesture(perform: onSelect)
    }
}

// Custom (non-native) segmented tab bar — replaces button-tab clusters and Picker.
struct SegmentedTabs<Tab: Hashable>: View {
    let tabs: [Tab]
    @Binding var selection: Tab
    var label: (Tab) -> String
    var accent: Bool = true
    var body: some View {
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

// Shared presentational row for file/folder trees (diff tree, config tree).
struct TreeRow: View {
    var depth: Int
    var indent: (Int) -> CGFloat = { CGFloat($0) * 14 + 10 }
    var isDir: Bool
    var expanded: Bool = false
    var name: String
    var leadingSymbol: String? = nil
    var marker: (text: String, color: Color)? = nil
    var selected: Bool = false
    var selectionColor: Color = Theme.sel
    var nameColor: Color = Theme.fg
    var nameWeight: Font.Weight = .regular
    var tooltip: String? = nil
    let onTap: () -> Void

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: 5) {
                if isDir {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 8.5, weight: .semibold)).foregroundStyle(Theme.dim).frame(width: 10)
                }
                if let m = marker {
                    Text(m.text).font(Theme.mono(9.5, .bold)).foregroundStyle(m.color).frame(width: 12)
                }
                if let sym = leadingSymbol {
                    Image(systemName: sym).font(.system(size: 10.5)).foregroundStyle(Theme.fgMuted)
                }
                Text(name).font(.system(size: 11.5, weight: nameWeight)).foregroundStyle(nameColor)
                    .lineLimit(1).truncationMode(.middle)
                Spacer(minLength: 4)
            }
            .padding(.leading, indent(depth)).padding(.horizontal, 8).padding(.vertical, 4)
            .background(selected ? selectionColor : .clear, in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
            .modifier(OptionalTooltip(tooltip))
        }.buttonStyle(.plain)
    }
}

private struct OptionalTooltip: ViewModifier {
    let text: String?
    init(_ text: String?) { self.text = text }
    func body(content: Content) -> some View {
        if let t = text { content.tooltip(t) } else { content }
    }
}

/// A "?" badge that explains a setting on hover. Deliberately inert on click —
/// it is an affordance for the explanation, not a control.
///
/// Uses the system `help` tooltip, not the app's: TooltipOverlay is mounted once
/// in RootView, so the in-app bubble would render in the main window while this
/// badge lives in a separate one.
struct HelpHint: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Image(systemName: "questionmark.circle.fill")
            .font(.system(size: 11.5))
            .foregroundStyle(Theme.dim)
            .contentShape(Circle())
            .help(text)
            .accessibilityLabel(text)
    }
}
