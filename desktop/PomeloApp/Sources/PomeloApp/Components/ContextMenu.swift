import SwiftUI
import AppKit

// A themed, cursor-anchored context menu. Unlike SwiftUI `.popover`, it has no arrow and
// opens exactly at the mouse location, in a borderless floating panel — the way a real
// context menu behaves. Reused by the file tree and the code editor.
@MainActor
enum ContextMenu {
    private static var panel: NSPanel?
    private static var hosted: NSView?
    private static var monitors: [Any] = []

    /// Present `content` as a floating panel with its top-left at `point` (screen coordinates).
    /// Used where there's no scrolling surface to pin to (e.g. the file tree rows).
    static func show<Content: View>(at point: NSPoint, @ViewBuilder _ content: (@escaping () -> Void) -> Content) {
        dismiss()

        let host = NSHostingView(rootView: AnyView(content { dismiss() }))
        host.layoutSubtreeIfNeeded()
        host.setFrameSize(host.fittingSize)

        let panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: host.fittingSize),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.isFloatingPanel = true
        panel.level = .popUpMenu
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isOpaque = false
        panel.hidesOnDeactivate = true
        panel.contentView = host

        positionTopLeft(panel, at: point)
        panel.orderFrontRegardless()
        Self.panel = panel
        installDismissMonitors()
    }

    /// Present `content` as a subview of `container` (a flipped, scrolling document view) with its
    /// top-left at `point` in that view's coordinates. Because the menu is part of the scrolled
    /// content it tracks scrolling perfectly (Zed's in-editor menu) and clips when out of view.
    static func show<Content: View>(
        inView container: NSView,
        at point: NSPoint,
        @ViewBuilder _ content: (@escaping () -> Void) -> Content
    ) {
        dismiss()

        let host = NSHostingView(rootView: AnyView(content { dismiss() }))
        host.layoutSubtreeIfNeeded()
        let size = host.fittingSize
        host.setFrameSize(size)
        // Keep the menu fully inside the visible area: open downward from the click, flip up near the
        // bottom, then clamp so it can never be clipped (Zed opens upward near the bottom edge).
        let visible = container.visibleRect
        var origin = point
        if origin.y + size.height > visible.maxY {
            origin.y = point.y - size.height
        }
        origin.y = min(max(origin.y, visible.minY), max(visible.minY, visible.maxY - size.height))
        origin.x = min(max(origin.x, visible.minX), max(visible.minX, visible.maxX - size.width))
        host.setFrameOrigin(origin)
        container.addSubview(host)
        hosted = host
        installDismissMonitors()
    }

    private static func installDismissMonitors() {
        // A mouse-down inside the menu goes through (buttons handle it); anything else dismisses,
        // except clicks on a scroller — dragging the scrollbar is scrolling, not "click away".
        func isInsideMenu(_ event: NSEvent) -> Bool {
            if let panel, event.window == panel { return true }
            if let hit = event.window?.contentView?.hitTest(event.locationInWindow), hit is NSScroller { return true }
            if let hosted, event.window == hosted.window {
                let p = hosted.convert(event.locationInWindow, from: nil)
                return hosted.bounds.contains(p)
            }
            return false
        }
        monitors.append(NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { event in
            if !isInsideMenu(event) { dismiss() }
            return event
        } as Any)
        monitors.append(NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { _ in
            dismiss()
        } as Any)
        monitors.append(NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) { event in
            if event.keyCode == 53 { dismiss(); return nil } // esc
            return event
        } as Any)
    }

    static func dismiss() {
        monitors.forEach { NSEvent.removeMonitor($0) }
        monitors.removeAll()
        hosted?.removeFromSuperview()
        hosted = nil
        panel?.orderOut(nil)
        panel = nil
    }

    /// Standard container styling for a menu's items (width, padding, rounded background + border).
    static func container<V: View>(width: CGFloat = 214, @ViewBuilder _ content: () -> V) -> some View {
        VStack(alignment: .leading, spacing: 0) { content() }
            .frame(width: width)
            .padding(.vertical, 5)
            .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderSoft, lineWidth: 1))
    }

    private static func positionTopLeft(_ panel: NSPanel, at point: NSPoint) {
        var origin = NSPoint(x: point.x, y: point.y - panel.frame.height)
        if let screen = NSScreen.screens.first(where: { $0.frame.contains(point) }) ?? NSScreen.main {
            let vis = screen.visibleFrame
            origin.x = min(origin.x, vis.maxX - panel.frame.width)
            origin.x = max(origin.x, vis.minX)
            // Flip above the point if it would run off the bottom.
            if origin.y < vis.minY { origin.y = point.y }
            origin.y = min(origin.y, vis.maxY - panel.frame.height)
        }
        panel.setFrameOrigin(origin)
    }
}

// A single row in a ``ContextMenu``. Dismisses the menu before running its action.
struct ContextMenuRow: View {
    let label: String
    var symbol: String? = nil
    var destructive = false
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button {
            ContextMenu.dismiss()
            action()
        } label: {
            HStack(spacing: 8) {
                if let symbol {
                    Image(systemName: symbol).font(.system(size: 11)).frame(width: 15)
                        .foregroundStyle(destructive ? Theme.danger : Theme.fgMuted)
                }
                Text(label).font(.system(size: 12)).foregroundStyle(destructive ? Theme.danger : Theme.fg)
                Spacer(minLength: 12)
            }
            .padding(.horizontal, 10).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(hover ? Theme.sel : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

struct ContextMenuSeparator: View {
    var body: some View {
        Rectangle().fill(Theme.borderSoft).frame(height: 1).padding(.vertical, 4).padding(.horizontal, 8)
    }
}

// Catches right-clicks to trigger a custom menu while letting left-clicks fall through to the
// view below (hitTest only claims the event for the right button).
struct RightClickArea: NSViewRepresentable {
    let onRightClick: (NSPoint) -> Void
    func makeNSView(context: Context) -> NSView { V(onRightClick: onRightClick) }
    func updateNSView(_ nsView: NSView, context: Context) {}

    final class V: NSView {
        let onRightClick: (NSPoint) -> Void
        init(onRightClick: @escaping (NSPoint) -> Void) { self.onRightClick = onRightClick; super.init(frame: .zero) }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
        override func hitTest(_ point: NSPoint) -> NSView? {
            switch NSApp.currentEvent?.type {
            case .rightMouseDown, .rightMouseUp, .rightMouseDragged: return self
            default: return nil
            }
        }
        override func rightMouseDown(with event: NSEvent) {
            let screen = window?.convertPoint(toScreen: event.locationInWindow) ?? NSEvent.mouseLocation
            onRightClick(screen)
        }
    }
}
