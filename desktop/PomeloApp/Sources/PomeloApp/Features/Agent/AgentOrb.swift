import SwiftUI
import AppKit

struct AgentOrb: View {
    let color: Color
    var active: Bool = false
    var size: CGFloat = 10

    var body: some View {
        PulseOrb(color: NSColor(color), active: active, size: size)
            .frame(width: size, height: size)
    }
}

// The pulse runs on Core Animation, not a SwiftUI repeatForever: a continuous
// SwiftUI animation re-evaluates the view tree every frame and pins the display
// at max refresh, so many active orbs burned CPU and battery. A CALayer animation
// runs on the compositor and lets the system pick the frame rate.
struct PulseOrb: NSViewRepresentable {
    let color: NSColor
    var active: Bool
    var size: CGFloat

    func makeNSView(context: Context) -> OrbView { OrbView() }

    func updateNSView(_ view: OrbView, context: Context) {
        view.apply(color: color, size: size, active: active)
    }
}

final class OrbView: NSView {
    private let dot = CALayer()
    private let ring = CALayer()
    private var active = false
    private var size: CGFloat = 0

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.addSublayer(ring)
        layer?.addSublayer(dot)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func apply(color: NSColor, size: CGFloat, active: Bool) {
        let cg = color.cgColor
        if size != self.size {
            self.size = size
            let rect = CGRect(x: 0, y: 0, width: size, height: size)
            for l in [dot, ring] {
                l.bounds = rect
                l.cornerRadius = size / 2
                l.anchorPoint = CGPoint(x: 0.5, y: 0.5)
                l.position = CGPoint(x: size / 2, y: size / 2)
            }
        }
        dot.backgroundColor = cg
        ring.backgroundColor = cg
        guard active != self.active else { return }
        self.active = active
        if active { startPulse() } else { ring.removeAnimation(forKey: "pulse"); ring.opacity = 0 }
    }

    override var intrinsicContentSize: NSSize { NSSize(width: size, height: size) }
    override func layout() { super.layout(); dot.position = CGPoint(x: bounds.midX, y: bounds.midY); ring.position = dot.position }

    private func startPulse() {
        let scale = CABasicAnimation(keyPath: "transform.scale")
        scale.fromValue = 1; scale.toValue = 2.6
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 0.5; fade.toValue = 0
        let group = CAAnimationGroup()
        group.animations = [scale, fade]
        group.duration = 1.1
        group.repeatCount = .infinity
        group.timingFunction = CAMediaTimingFunction(name: .easeOut)
        group.isRemovedOnCompletion = false
        ring.add(group, forKey: "pulse")
    }
}
