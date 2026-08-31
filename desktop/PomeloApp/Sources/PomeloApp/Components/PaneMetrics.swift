import SwiftUI

// The workspace split is user-resizable down to 320pt, so any master/detail pane
// has to survive widths a fixed two-column layout can't. These are the shared
// thresholds and the drill-down chrome that go with them.
enum PaneMetrics {
    // Below this a master/detail pane stacks into one drill-down column.
    static let stackWidth: CGFloat = 620
    // Detail never shrinks past this while side-by-side; the master gives up room first.
    static let minDetail: Double = 340
    // Below this the diff file tree floats over the diff instead of taking a column.
    static let diffTreeOverlayWidth: CGFloat = 520
}

private struct PaneNarrowKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    // Set by the pane that owns the width; children below it lay out compactly
    // without each re-measuring in its own GeometryReader.
    var paneNarrow: Bool {
        get { self[PaneNarrowKey.self] }
        set { self[PaneNarrowKey.self] = newValue }
    }
}

// Drill-down header for a stacked pane: back to the list, plus what you drilled into.
struct BackBar: View {
    @EnvironmentObject var theme: ThemeManager
    let title: String
    var back: String = "Back"
    let onBack: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Button(action: onBack) {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left").font(.system(size: 10, weight: .semibold))
                    Text(back).font(.system(size: 11.5))
                }
                .foregroundStyle(Theme.accent)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .keyboardShortcut(.escape, modifiers: [])
            if !title.isEmpty {
                Text(title).font(Theme.mono(11, .medium)).foregroundStyle(Theme.fg)
                    .lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 7)
        .background(Theme.bgSoft)
    }
}

// Wraps chips/badges onto as many rows as the width needs instead of clipping them.
struct WrapHStack: Layout {
    var spacing: CGFloat = 5
    var lineSpacing: CGFloat = 4

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxW = proposal.width ?? .infinity
        let rows = layout(subviews, maxW: maxW)
        let h = rows.reduce(0) { $0 + $1.height } + lineSpacing * CGFloat(max(0, rows.count - 1))
        let w = rows.map(\.width).max() ?? 0
        return CGSize(width: min(w, maxW == .infinity ? w : maxW), height: h)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in layout(subviews, maxW: bounds.width) {
            var x = bounds.minX
            for i in row.indices {
                let size = subviews[i].sizeThatFits(.unspecified)
                subviews[i].place(at: CGPoint(x: x, y: y + (row.height - size.height) / 2),
                                  anchor: .topLeading, proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + lineSpacing
        }
    }

    private func layout(_ subviews: Subviews, maxW: CGFloat) -> [WrapRow] {
        WrapRow.pack(subviews.indices.map { subviews[$0].sizeThatFits(.unspecified) },
                     maxW: maxW, spacing: spacing)
    }
}

// Row packing kept free of SwiftUI's Subviews so it can be tested directly.
struct WrapRow: Equatable {
    var indices: [Int] = []
    var width: CGFloat = 0
    var height: CGFloat = 0

    // Greedy: an item that doesn't fit starts a new row. An item wider than maxW
    // still gets its own row rather than being dropped.
    static func pack(_ sizes: [CGSize], maxW: CGFloat, spacing: CGFloat) -> [WrapRow] {
        var rows: [WrapRow] = []
        var row = WrapRow()
        for (i, s) in sizes.enumerated() {
            let next = row.indices.isEmpty ? s.width : row.width + spacing + s.width
            if !row.indices.isEmpty && next > maxW {
                rows.append(row)
                row = WrapRow(indices: [i], width: s.width, height: s.height)
            } else {
                row.indices.append(i)
                row.width = next
                row.height = max(row.height, s.height)
            }
        }
        if !row.indices.isEmpty { rows.append(row) }
        return rows
    }
}
