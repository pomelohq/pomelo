import SwiftUI

struct AgentOrb: View {
    let color: Color
    var active: Bool = false
    var size: CGFloat = 10
    private let period = 1.3

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .overlay {
                if active {
                    TimelineView(.animation) { tl in
                        Canvas { ctx, sz in
                            let c = CGPoint(x: sz.width / 2, y: sz.height / 2)
                            let phase = tl.date.timeIntervalSinceReferenceDate
                                .truncatingRemainder(dividingBy: period) / period
                            let r = size / 2 + CGFloat(phase) * size * 1.1
                            let rect = CGRect(x: c.x - r, y: c.y - r, width: r * 2, height: r * 2)
                            ctx.fill(Circle().path(in: rect),
                                     with: .color(color.opacity((1 - phase) * 0.5)))
                        }
                        .frame(width: size * 3.2, height: size * 3.2)
                    }
                    .allowsHitTesting(false)
                }
            }
            .animation(.easeInOut(duration: 0.3), value: color)
    }
}

func agentOrbColor(_ state: String) -> Color {
    switch state {
    case "idle":           return Theme.ok
    case "thinking":       return Theme.warn
    case "tool_use":       return Theme.tool
    case "compacting":     return Theme.wsAccent
    case "awaiting_input": return Theme.danger
    default:               return Theme.dim
    }
}

func agentOrbActive(_ state: String) -> Bool {
    ["thinking", "tool_use", "compacting", "awaiting_input"].contains(state)
}
