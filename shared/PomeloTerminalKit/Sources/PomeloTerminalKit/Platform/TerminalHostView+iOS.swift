#if os(iOS)
import UIKit
import QuartzCore

// UIKit host around the shared GPU `TerminalRenderer`. Read-only mirror: it renders the
// PTY byte stream on the GPU (smooth scroll + Nerd Font icons) and reports geometry
// changes; all keyboard/scroll input is routed to the remote PTY by the app, so this
// view needs no first responder. App-specific wiring lives in the SwiftUI representable.
public final class MetalTerminalHostView: UIView {
    public let renderer = TerminalRenderer()
    private var displayLink: CADisplayLink?
    public var onResize: ((Int, Int) -> Void)?

    public override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        layer.addSublayer(renderer.metalLayer)
        renderer.onResize = { [weak self] c, r in self?.onResize?(c, r) }
    }
    public required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private var currentScale: CGFloat { let s = window?.screen.scale ?? traitCollection.displayScale; return s > 0 ? s : 2 }

    public override func layoutSubviews() {
        super.layoutSubviews()
        renderer.metalLayer.frame = bounds
        renderer.updateGeometry(pointSize: bounds.size, scale: currentScale)
    }

    public override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil, displayLink == nil {
            let link = CADisplayLink(target: renderer, selector: #selector(TerminalRenderer.renderTick))
            link.add(to: .main, forMode: .common)
            displayLink = link
        } else if window == nil {
            displayLink?.invalidate(); displayLink = nil
            renderer.teardown()
        }
    }

    public func feed(_ bytes: [UInt8]) { renderer.feed(bytes) }
    public func feed(_ bytes: ArraySlice<UInt8>) { renderer.feed(bytes) }
    public func setFont(family: String, size: CGFloat) { renderer.setFont(family: family, size: size); setNeedsLayout() }
    public func applyColors(fg: SIMD4<Float>, bg: SIMD4<Float>) { renderer.setColors(fg: fg, bg: bg) }
}
#endif
