import SwiftUI

// Claude subscription usage (5h session + weekly) as a compact meter. State is owned
// by UsageStore (one poller, off-main, delta-guarded); the chip only renders it, and a
// click drops a styled card with each window's remaining budget, reset, and the account.
struct AgentUsageChip: View {
    @EnvironmentObject private var theme: ThemeManager
    @StateObject private var store = UsageStore.shared
    @State private var open = false
    private var u: ClaudeUsage? { store.usage }

    var body: some View {
        let _ = theme.mode
        return Button { open.toggle() } label: { chip }
            .buttonStyle(.plain)
            .dropdownMenu(isPresented: $open, alignment: .topTrailing, drop: 26) { card }
            .task { store.start() }
    }

    @ViewBuilder private var chip: some View {
        if let u, u.ok == true {
            let s = u.session?.pct ?? 0, w = u.weekly?.pct ?? 0
            HStack(spacing: 6) {
                claudeMark(11)
                bar(s, width: 40)
                (Text(pct(s)).foregroundColor(Theme.fg)
                    + Text(" 5h · ").foregroundColor(Theme.muted)
                    + Text(pct(w)).foregroundColor(Theme.fg)
                    + Text(" wk").foregroundColor(Theme.muted))
                    .font(Theme.mono(10.5))
            }
            .padding(.horizontal, 7).padding(.vertical, 3)
            .background(open ? Theme.hover : Theme.chip, in: Capsule())
            .contentShape(Capsule())
        } else {
            HStack(spacing: 5) {
                claudeMark(11).opacity(0.5)
                Text(u == nil ? "usage…" : "usage —").font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
            }
            .padding(.horizontal, 7).padding(.vertical, 3)
            .background(Theme.chip, in: Capsule())
            .contentShape(Capsule())
        }
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 6) {
                claudeMark(13)
                Text("Claude usage").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.fg)
                Spacer(minLength: 16)
                if let plan = u?.account?.plan, !plan.isEmpty {
                    Text(plan.capitalized).font(Theme.mono(9.5, .semibold)).foregroundStyle(Theme.accent)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Theme.accentSoft, in: Capsule())
                }
            }
            if let u, u.ok == true {
                window("Session", u.session)
                window("Weekly", u.weekly)
            } else {
                Text(u == nil ? "Loading usage…" : "Usage unavailable").font(.system(size: 11)).foregroundStyle(Theme.dim)
            }
            if let a = u?.account, (a.email?.isEmpty == false || a.org?.isEmpty == false) {
                Divider().overlay(Theme.borderSoft)
                VStack(alignment: .leading, spacing: 1) {
                    if let e = a.email, !e.isEmpty { Text(e).font(Theme.mono(10.5)).foregroundStyle(Theme.fgMuted) }
                    if let o = a.org, !o.isEmpty { Text(o).font(Theme.mono(9.5)).foregroundStyle(Theme.dim) }
                }
            }
        }
        .padding(13)
        .frame(width: 250, alignment: .leading)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.border))
    }

    private func window(_ title: String, _ w: ClaudeUsage.Win?) -> some View {
        let used = w?.pct ?? 0
        return VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.fg)
            bar(used, width: nil)
            HStack {
                Text("\(Int(used.rounded()))% used").font(Theme.mono(10)).foregroundStyle(Theme.fgMuted)
                Spacer()
                if let r = w?.resets_at, r > 0 {
                    Text("Resets \(relative(r))").font(Theme.mono(10)).foregroundStyle(Theme.dim)
                        .help("resets \(clock(r))")
                }
            }
        }
    }

    private func claudeMark(_ size: CGFloat) -> some View {
        Image("claude", bundle: .module).resizable().interpolation(.high)
            .scaledToFit().frame(width: size, height: size)
    }

    private func pct(_ p: Double) -> String { "\(Int(p.rounded()))%" }

    @ViewBuilder private func bar(_ used: Double, width: CGFloat?) -> some View {
        let frac = min(1, max(0, used / 100))
        if let width {
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.dim.opacity(0.5)).frame(width: width, height: 5)
                Capsule().fill(color(used)).frame(width: width * frac, height: 5)
            }
        } else {
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.dim.opacity(0.4))
                    Capsule().fill(color(used)).frame(width: geo.size.width * frac)
                }
            }.frame(height: 6)
        }
    }

    private func color(_ used: Double) -> Color { used >= 90 ? Theme.danger : used >= 70 ? Theme.warn : Theme.ok }

    private func clock(_ unix: Int64) -> String {
        let f = DateFormatter(); f.dateFormat = "EEE HH:mm"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(unix)))
    }

    private func relative(_ unix: Int64) -> String {
        let secs = unix - Int64(Date().timeIntervalSince1970)
        if secs <= 0 { return "now" }
        let d = secs / 86400, h = (secs % 86400) / 3600, m = (secs % 3600) / 60
        if d > 0 { return "in \(d)d \(h)h" }
        return h > 0 ? "in \(h)h \(m)m" : "in \(m)m"
    }
}
