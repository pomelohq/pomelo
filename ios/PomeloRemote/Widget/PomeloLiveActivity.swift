import SwiftUI
import WidgetKit
import ActivityKit

@available(iOS 16.1, *)
struct PomeloLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: PomeloAgentAttributes.self) { context in
            lockScreen(context.state)
                .padding(14)
                .activityBackgroundTint(Color.black.opacity(0.5))
                .activitySystemActionForegroundColor(.white)
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    HStack(spacing: 8) {
                        orb(context.state.state)
                        Text("\(context.state.activeCount)").font(.system(size: 16, weight: .bold).monospacedDigit())
                    }.padding(.leading, 4)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(agentStateLabel(context.state.state))
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(agentStateColor(context.state.state))
                        .lineLimit(1).fixedSize()
                        .padding(.trailing, 4)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack(spacing: 6) {
                            Text("claude").font(.system(size: 12, weight: .semibold, design: .monospaced))
                                .foregroundStyle(.white.opacity(0.55))
                            Text(context.state.headline).font(.system(size: 13, weight: .medium)).lineLimit(1)
                            Spacer()
                        }
                        usageTag(context.state)
                    }
                }
            } compactLeading: {
                orb(context.state.state)
            } compactTrailing: {
                Text("\(context.state.activeCount)").font(.system(size: 13, weight: .bold).monospacedDigit())
            } minimal: {
                orb(context.state.state)
            }
            .widgetURL(URL(string: "pomelo://agents"))
        }
    }

    private func lockScreen(_ s: PomeloAgentAttributes.ContentState) -> some View {
        HStack(spacing: 12) {
            orb(s.state, size: 12)
            VStack(alignment: .leading, spacing: 2) {
                Text(s.headline).font(.system(size: 15, weight: .semibold)).foregroundStyle(.white).lineLimit(1)
                HStack(spacing: 5) {
                    Text("claude").font(.system(size: 11, weight: .semibold, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.55))
                    Text(agentStateLabel(s.state)).font(.system(size: 12)).foregroundStyle(agentStateColor(s.state))
                }
                usageTag(s)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 1) {
                Text("\(s.activeCount)").font(.system(size: 20, weight: .bold).monospacedDigit()).foregroundStyle(.white)
                Text("active").font(.system(size: 10)).foregroundStyle(.white.opacity(0.6))
            }
        }
    }

    private func orb(_ state: String, size: CGFloat = 10) -> some View {
        Circle().fill(agentStateColor(state)).frame(width: size, height: size)
    }

    @ViewBuilder private func usageTag(_ s: PomeloAgentAttributes.ContentState) -> some View {
        if s.sessionPct > 0 || s.weeklyPct > 0 {
            HStack(spacing: 12) {
                usageItem("5h", s.sessionPct)
                usageItem("7d", s.weeklyPct)
            }
        }
    }

    private func usageItem(_ label: String, _ pct: Int) -> some View {
        HStack(spacing: 5) {
            Text(label).font(.system(size: 10, weight: .semibold, design: .monospaced))
                .foregroundStyle(.white.opacity(0.5))
            usageBar(pct)
            Text("\(pct)%").font(.system(size: 10, weight: .medium, design: .monospaced))
                .monospacedDigit().foregroundStyle(.white.opacity(0.8))
        }
    }

    private func usageBar(_ pct: Int, width: CGFloat = 40) -> some View {
        let frac = min(1, max(0, Double(pct) / 100))
        return ZStack(alignment: .leading) {
            Capsule().fill(.white.opacity(0.18)).frame(width: width, height: 4)
            Capsule().fill(usageColor(pct)).frame(width: max(2, width * frac), height: 4)
        }
    }

    private func usageColor(_ pct: Int) -> Color {
        pct >= 90 ? .red : (pct >= 70 ? .orange : Color(.sRGB, red: 0.3, green: 0.8, blue: 0.4))
    }
}
