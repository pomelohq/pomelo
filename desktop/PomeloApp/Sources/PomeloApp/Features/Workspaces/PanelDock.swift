import SwiftUI

// A resizable, toggleable edge dock hosting one active panel, modeled on Zed's Dock (crates/workspace/src/dock.rs):
// a Dock has a position, an open/closed state, an active panel, and a draggable resize handle. Panels are supplied by
// the caller; this owns only the geometry (open + size + resize). Left/right resize width, bottom resizes height.
struct PanelDock<Content: View>: View {
    enum Edge { case left, right, bottom }

    let edge: Edge
    let isOpen: Bool
    @Binding var size: Double
    let minSize: Double
    let maxSize: Double
    @ViewBuilder let content: () -> Content

    var body: some View {
        if isOpen {
            switch edge {
            case .left:
                HStack(spacing: 0) { panel; handle }
            case .right:
                HStack(spacing: 0) { handle; panel }
            case .bottom:
                VStack(spacing: 0) { handle; panel }
            }
        }
    }

    private var panel: some View {
        content()
            .frame(width: edge == .bottom ? nil : clamped, height: edge == .bottom ? clamped : nil)
            .frame(maxWidth: edge == .bottom ? .infinity : nil, maxHeight: edge == .bottom ? nil : .infinity)
            .background(Theme.bg)
            .clipped()
    }

    private var handle: some View {
        SplitHandle(axis: edge == .bottom ? .vertical : .horizontal,
                    value: $size, min: minSize, max: maxSize,
                    invert: edge == .right)
    }

    private var clamped: CGFloat { CGFloat(min(max(size, minSize), maxSize)) }
}
