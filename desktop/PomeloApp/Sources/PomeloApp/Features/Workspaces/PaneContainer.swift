import SwiftUI
import AppKit
import CodeEditTextView

struct PaneContainerHost: NSViewRepresentable {
    let root: PaneNode
    let structureSignature: String
    let primaryToken: String
    let focusedLeaf: String
    let dragActive: Bool
    let leafContent: (PaneNode) -> AnyView
    let onFraction: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> PaneContainerView {
        let view = PaneContainerView()
        view.makeContent = leafContent
        view.onCommitFraction = onFraction
        context.coordinator.view = view
        context.coordinator.installWindowResizeFreeze()
        view.setRoot(root)
        context.coordinator.applyTargeted(root: root, primaryToken: primaryToken, focusedLeaf: focusedLeaf, dragActive: dragActive)
        return view
    }

    func updateNSView(_ view: PaneContainerView, context: Context) {
        view.makeContent = leafContent
        view.onCommitFraction = onFraction
        if structureSignature != context.coordinator.signature {
            context.coordinator.signature = structureSignature
            view.setRoot(root)
        } else {
            view.root = root
        }
        context.coordinator.applyTargeted(root: root, primaryToken: primaryToken, focusedLeaf: focusedLeaf, dragActive: dragActive)
    }

    @MainActor
    final class Coordinator {
        weak var view: PaneContainerView?
        var signature = ""
        private var primaryToken = ""
        private var focused = "leaf-0"
        private var dragActive = false
        private var resizeObservers: [NSObjectProtocol] = []

        func applyTargeted(root: PaneNode, primaryToken: String, focusedLeaf: String, dragActive: Bool) {
            guard let view else { return }
            if primaryToken != self.primaryToken { self.primaryToken = primaryToken; view.refreshContent("leaf-0") }
            if focusedLeaf != self.focused {
                let old = self.focused; self.focused = focusedLeaf
                view.refreshContent(old); view.refreshContent(focusedLeaf)
            }
            if dragActive != self.dragActive {
                self.dragActive = dragActive
                for leaf in root.leaves { view.refreshContent(leaf.id) }
            }
        }

        func installWindowResizeFreeze() {
            let nc = NotificationCenter.default
            resizeObservers.append(nc.addObserver(forName: NSWindow.willStartLiveResizeNotification, object: nil, queue: .main) { _ in
                CETextViewSuppressLayout = true
            })
            resizeObservers.append(nc.addObserver(forName: NSWindow.didEndLiveResizeNotification, object: nil, queue: .main) { _ in
                CETextViewSuppressLayout = false
                NotificationCenter.default.post(name: .ceForceRelayout, object: nil)
            })
        }

        deinit { resizeObservers.forEach { NotificationCenter.default.removeObserver($0) } }
    }
}

final class PaneContainerView: NSView {
    var root: PaneNode?
    var makeContent: (PaneNode) -> AnyView = { _ in AnyView(Color.clear) }
    var onCommitFraction: () -> Void = {}

    static let minW: CGFloat = 200
    static let minH: CGFloat = 120
    static let divider: CGFloat = 1
    static let hitSlop: CGFloat = 5

    private var leafHosts: [String: NSHostingView<AnyView>] = [:]
    private let dividerColor = NSColor.separatorColor

    override var isFlipped: Bool { true }

    func setRoot(_ node: PaneNode?) {
        root = node
        syncHosts()
        needsLayout = true
        needsDisplay = true
        window?.invalidateCursorRects(for: self)
    }

    private func syncHosts() {
        guard let root else {
            leafHosts.values.forEach { $0.removeFromSuperview() }
            leafHosts.removeAll()
            return
        }
        let live = Set(root.leaves.map(\.id))
        for (id, host) in leafHosts where !live.contains(id) {
            host.removeFromSuperview(); leafHosts[id] = nil
        }
        for leaf in root.leaves where leafHosts[leaf.id] == nil {
            let host = NSHostingView(rootView: makeContent(leaf))
            host.sizingOptions = []
            leafHosts[leaf.id] = host
            addSubview(host)
        }
    }

    func refreshContent(_ leafID: String) {
        guard let node = root?.leaf(withID: leafID) else { return }
        leafHosts[leafID]?.rootView = makeContent(node)
    }

    override func layout() {
        super.layout()
        guard let root else { return }
        layoutNode(root, bounds)
    }

    private func layoutNode(_ node: PaneNode, _ rect: CGRect) {
        switch node.kind {
        case .leaf:
            leafHosts[node.id]?.frame = rect
        case .branch(let axis, let a, let b, let frac):
            let (first, second, horizontal) = split(a, b, axis: axis, frac: frac, rect: rect)
            if horizontal {
                layoutNode(a, CGRect(x: rect.minX, y: rect.minY, width: first, height: rect.height))
                layoutNode(b, CGRect(x: rect.minX + first + Self.divider, y: rect.minY, width: second, height: rect.height))
            } else {
                layoutNode(a, CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: first))
                layoutNode(b, CGRect(x: rect.minX, y: rect.minY + first + Self.divider, width: rect.width, height: second))
            }
        }
    }

    private func split(_ a: PaneNode, _ b: PaneNode, axis: PaneNode.Axis, frac: CGFloat, rect: CGRect) -> (first: CGFloat, second: CGFloat, horizontal: Bool) {
        let horizontal = axis == .horizontal
        let avail = max(0, (horizontal ? rect.width : rect.height) - Self.divider)
        var fMin = Self.minExtent(a, along: axis), sMin = Self.minExtent(b, along: axis)
        if fMin + sMin > avail, fMin + sMin > 0 { let k = avail / (fMin + sMin); fMin *= k; sMin *= k }
        var first = (avail * frac).rounded()
        first = min(max(first, fMin), avail - sMin)
        return (first, avail - first, horizontal)
    }

    static func minExtent(_ node: PaneNode, along axis: PaneNode.Axis) -> CGFloat {
        switch node.kind {
        case .leaf:
            return axis == .horizontal ? minW : minH
        case .branch(let ax, let a, let b, _):
            let ma = minExtent(a, along: axis), mb = minExtent(b, along: axis)
            return ax == axis ? ma + mb + divider : max(ma, mb)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let root else { return }
        dividerColor.setFill()
        drawDividers(root, bounds)
    }

    private func drawDividers(_ node: PaneNode, _ rect: CGRect) {
        guard case .branch(let axis, let a, let b, let frac) = node.kind else { return }
        let (first, _, horizontal) = split(a, b, axis: axis, frac: frac, rect: rect)
        if horizontal {
            CGRect(x: rect.minX + first, y: rect.minY, width: Self.divider, height: rect.height).fill()
            drawDividers(a, CGRect(x: rect.minX, y: rect.minY, width: first, height: rect.height))
            drawDividers(b, CGRect(x: rect.minX + first + Self.divider, y: rect.minY, width: rect.width - first - Self.divider, height: rect.height))
        } else {
            CGRect(x: rect.minX, y: rect.minY + first, width: rect.width, height: Self.divider).fill()
            drawDividers(a, CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: first))
            drawDividers(b, CGRect(x: rect.minX, y: rect.minY + first + Self.divider, width: rect.width, height: rect.height - first - Self.divider))
        }
    }

    private func dividerHit(_ node: PaneNode, _ rect: CGRect, _ p: CGPoint) -> (node: PaneNode, rect: CGRect, horizontal: Bool)? {
        guard case .branch(let axis, let a, let b, let frac) = node.kind else { return nil }
        let (first, second, horizontal) = split(a, b, axis: axis, frac: frac, rect: rect)
        let dividerRect: CGRect, aRect: CGRect, bRect: CGRect
        if horizontal {
            dividerRect = CGRect(x: rect.minX + first - Self.hitSlop, y: rect.minY, width: Self.divider + Self.hitSlop * 2, height: rect.height)
            aRect = CGRect(x: rect.minX, y: rect.minY, width: first, height: rect.height)
            bRect = CGRect(x: rect.minX + first + Self.divider, y: rect.minY, width: second, height: rect.height)
        } else {
            dividerRect = CGRect(x: rect.minX, y: rect.minY + first - Self.hitSlop, width: rect.width, height: Self.divider + Self.hitSlop * 2)
            aRect = CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: first)
            bRect = CGRect(x: rect.minX, y: rect.minY + first + Self.divider, width: rect.width, height: second)
        }
        if dividerRect.contains(p) { return (node, rect, horizontal) }
        if aRect.contains(p) { return dividerHit(a, aRect, p) }
        if bRect.contains(p) { return dividerHit(b, bRect, p) }
        return nil
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard let root else { return super.hitTest(point) }
        if dividerHit(root, bounds, convert(point, from: superview)) != nil { return self }
        return super.hitTest(point)
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard let root, let hit = dividerHit(root, bounds, p) else { return }
        CETextViewSuppressLayout = true
        window?.trackEvents(matching: [.leftMouseDragged, .leftMouseUp], timeout: .infinity, mode: .eventTracking) { ev, stop in
            guard let ev else { stop.pointee = true; return }
            if ev.type == .leftMouseUp { stop.pointee = true; return }
            let cur = self.convert(ev.locationInWindow, from: nil)
            let axisLen = (hit.horizontal ? hit.rect.width : hit.rect.height) - Self.divider
            guard axisLen > 0 else { return }
            let pos = hit.horizontal ? (cur.x - hit.rect.minX) : (cur.y - hit.rect.minY)
            hit.node.setFraction(pos / axisLen)
            self.needsLayout = true
            self.needsDisplay = true
            self.layoutSubtreeIfNeeded()
        }
        CETextViewSuppressLayout = false
        NotificationCenter.default.post(name: .ceForceRelayout, object: nil)
        window?.invalidateCursorRects(for: self)
        onCommitFraction()
    }

    override func resetCursorRects() {
        guard let root else { return }
        addDividerCursorRects(root, bounds)
    }

    private func addDividerCursorRects(_ node: PaneNode, _ rect: CGRect) {
        guard case .branch(let axis, let a, let b, let frac) = node.kind else { return }
        let (first, second, horizontal) = split(a, b, axis: axis, frac: frac, rect: rect)
        if horizontal {
            addCursorRect(CGRect(x: rect.minX + first - Self.hitSlop, y: rect.minY, width: Self.divider + Self.hitSlop * 2, height: rect.height), cursor: .resizeLeftRight)
            addDividerCursorRects(a, CGRect(x: rect.minX, y: rect.minY, width: first, height: rect.height))
            addDividerCursorRects(b, CGRect(x: rect.minX + first + Self.divider, y: rect.minY, width: second, height: rect.height))
        } else {
            addCursorRect(CGRect(x: rect.minX, y: rect.minY + first - Self.hitSlop, width: rect.width, height: Self.divider + Self.hitSlop * 2), cursor: .resizeUpDown)
            addDividerCursorRects(a, CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: first))
            addDividerCursorRects(b, CGRect(x: rect.minX, y: rect.minY + first + Self.divider, width: rect.width, height: second))
        }
    }
}
