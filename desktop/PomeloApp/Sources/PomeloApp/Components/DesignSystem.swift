import SwiftUI

// Portable primitives (Spinner, IconButton, Card, StatusPill, SelectableRow,
// SegmentedTabs, HelpHint) now live in the shared PomeloUI package (re-exported via
// Core/Theme.swift). This file keeps the macOS-specific rows that couple to the app's
// tooltip system and section headers.

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
    var hoverTrailing: AnyView? = nil
    let onTap: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 5) {
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
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .modifier(OptionalTooltip(tooltip))
            if hovering, let hoverTrailing {
                hoverTrailing
            }
        }
        .padding(.leading, indent(depth)).padding(.horizontal, 8).padding(.vertical, 4)
        .background(selected ? selectionColor : .clear, in: RoundedRectangle(cornerRadius: 6))
        .onHover { hovering = $0 }
    }
}

private struct OptionalTooltip: ViewModifier {
    let text: String?
    init(_ text: String?) { self.text = text }
    func body(content: Content) -> some View {
        if let t = text { content.tooltip(t) } else { content }
    }
}
