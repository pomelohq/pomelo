import SwiftUI

// Claude subscription usage (5h session + weekly) as a compact meter. State is owned
// by UsageStore (one poller, off-main, delta-guarded); the chip only renders it.
struct ClaudeUsageChip: View {
    @EnvironmentObject private var theme: ThemeManager
    @StateObject private var store = UsageStore.shared
    private var u: ClaudeUsage? { store.usage }

    var body: some View {
        let _ = theme.mode
        return Group {
            if let u, u.ok == true {
                let s = u.session?.pct ?? 0, w = u.weekly?.pct ?? 0
                HStack(spacing: 6) {
                    Image(systemName: "sparkle").font(.system(size: 10)).foregroundStyle(Theme.accent)
                    bar(s)
                    (Text(pct(s)).foregroundColor(Theme.fg)
                        + Text(" 5h · ").foregroundColor(Theme.muted)
                        + Text(pct(w)).foregroundColor(Theme.fg)
                        + Text(" wk").foregroundColor(Theme.muted))
                        .font(Theme.mono(10.5))
                }
                .padding(.horizontal, 7).padding(.vertical, 3)
                .background(Theme.chip, in: Capsule())
                .help(tooltip(u))
            } else {
                HStack(spacing: 5) {
                    Image(systemName: "sparkle").font(.system(size: 10)).foregroundStyle(Theme.dim)
                    Text(u == nil ? "usage…" : "usage —").font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
                }
                .padding(.horizontal, 7).padding(.vertical, 3)
                .background(Theme.chip, in: Capsule())
                .help(u == nil ? "Loading Claude usage…" : "Claude usage unavailable: \(u?.error?.isEmpty == false ? u!.error! : "unknown")")
            }
        }
        .task { store.start() }
    }

    private func pct(_ p: Double) -> String { "\(Int(p.rounded()))%" }

    private func bar(_ p: Double) -> some View {
        ZStack(alignment: .leading) {
            Capsule().fill(Theme.dim.opacity(0.5)).frame(width: 40, height: 5)
            Capsule().fill(color(p)).frame(width: 40 * min(1, max(0, p / 100)), height: 5)
        }
    }

    private func color(_ p: Double) -> Color { p >= 90 ? Theme.danger : p >= 70 ? Theme.warn : Theme.ok }

    private func tooltip(_ u: ClaudeUsage) -> String {
        "Claude usage\n5h session: \(pct(u.session?.pct ?? 0)) \(resetIn(u.session?.resets_at ?? 0))\nWeekly: \(pct(u.weekly?.pct ?? 0)) \(resetIn(u.weekly?.resets_at ?? 0))"
    }

    private func resetIn(_ unix: Int64) -> String {
        guard unix > 0 else { return "" }
        let secs = unix - Int64(Date().timeIntervalSince1970)
        if secs <= 0 { return "(resetting)" }
        let h = secs / 3600, m = (secs % 3600) / 60
        return h > 0 ? "(resets in \(h)h \(m)m)" : "(resets in \(m)m)"
    }
}
