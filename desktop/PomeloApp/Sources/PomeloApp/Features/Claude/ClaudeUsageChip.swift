import SwiftUI

struct ClaudeUsage: Decodable {
    struct Win: Decodable { var pct: Double = 0; var resets_at: Int64 = 0 }
    var ok = false
    var session = Win()
    var weekly = Win()
}

// Claude subscription usage (5h session + weekly) shown as a compact meter, polled
// from the same OAuth usage endpoint Claude Code reads for its status line.
struct ClaudeUsageChip: View {
    @State private var u: ClaudeUsage?

    var body: some View {
        Group {
            if let u {
                HStack(spacing: 6) {
                    Image(systemName: "sparkle").font(.system(size: 10)).foregroundStyle(Theme.accent)
                    bar(u.session.pct)
                    Text("\(pct(u.session.pct)) 5h · \(pct(u.weekly.pct)) wk")
                        .font(Theme.mono(10.5)).foregroundStyle(Theme.fgMuted)
                }
                .padding(.horizontal, 7).padding(.vertical, 3)
                .background(Theme.chip, in: Capsule())
                .help(tooltip(u))
            }
        }
        .task { await loop() }
    }

    private func pct(_ p: Double) -> String { "\(Int(p.rounded()))%" }

    private func bar(_ p: Double) -> some View {
        ZStack(alignment: .leading) {
            Capsule().fill(Theme.dim.opacity(0.25)).frame(width: 40, height: 5)
            Capsule().fill(color(p)).frame(width: 40 * min(1, max(0, p / 100)), height: 5)
        }
    }

    private func color(_ p: Double) -> Color { p >= 90 ? Theme.danger : p >= 70 ? Theme.warn : Theme.ok }

    private func tooltip(_ u: ClaudeUsage) -> String {
        "Claude usage\n5h session: \(pct(u.session.pct)) \(resetIn(u.session.resets_at))\nWeekly: \(pct(u.weekly.pct)) \(resetIn(u.weekly.resets_at))"
    }

    private func resetIn(_ unix: Int64) -> String {
        guard unix > 0 else { return "" }
        let secs = unix - Int64(Date().timeIntervalSince1970)
        if secs <= 0 { return "(resetting)" }
        let h = secs / 3600, m = (secs % 3600) / 60
        return h > 0 ? "(resets in \(h)h \(m)m)" : "(resets in \(m)m)"
    }

    private func loop() async {
        while !Task.isCancelled {
            let d = await Task.detached(priority: .utility) { PomCore.shared.claudeUsageData() }.value
            if let x = PomJSON.decode(ClaudeUsage.self, from: d), x.ok { u = x }
            try? await Task.sleep(nanoseconds: 60_000_000_000)
        }
    }
}
