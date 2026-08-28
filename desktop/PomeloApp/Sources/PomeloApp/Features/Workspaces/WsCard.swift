import SwiftUI

struct LeftCaret: Shape {
    func path(in r: CGRect) -> Path {
        var p = Path()
        p.move(to: CGPoint(x: r.minX, y: r.midY))
        p.addLine(to: CGPoint(x: r.maxX, y: r.minY))
        p.addLine(to: CGPoint(x: r.maxX, y: r.maxY))
        p.closeSubpath()
        return p
    }
}

struct PRPeekAnchorKey: PreferenceKey {
    static let defaultValue: [String: Anchor<CGRect>] = [:]
    static func reduce(value: inout [String: Anchor<CGRect>], nextValue: () -> [String: Anchor<CGRect>]) {
        value.merge(nextValue()) { a, _ in a }
    }
}

extension RootView {
    @ViewBuilder func prPeekPanel(_ id: String) -> some View {
        let prs = state.prsFor(id).filter { $0.pr != nil }
        if !prs.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(prs) { item in if let pr = item.pr { PRPopRow(item: item, pr: pr) } }
            }
            .frame(width: 340).padding(6)
            .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.border))
            .shadow(color: .black.opacity(0.35), radius: 12, y: 4)
            .onHover { h in if h { state.prPeekEnter(id) } else { state.prPeekLeave() } }
        }
    }
}

// Equatable over just the render-affecting inputs (closures ignored) so an
// `.equatable()` wrapper skips re-rendering rows whose data did not change on a
// global AppState tick — keeps the non-lazy sidebar smooth while scrolling.
extension WsCard: Equatable {
    static func == (l: WsCard, r: WsCard) -> Bool {
        l.ws == r.ws && l.selected == r.selected && l.agent == r.agent &&
        l.prs == r.prs && l.severity == r.severity && l.prsLoading == r.prsLoading &&
        l.pullOn == r.pullOn && l.pullIntervalSec == r.pullIntervalSec && l.pulling == r.pulling &&
        l.pulledAt == r.pulledAt && l.pullProgress == r.pullProgress && l.jira == r.jira &&
        l.themeMode == r.themeMode
    }
}

struct WsCard: View {
    @EnvironmentObject var theme: ThemeManager
    let ws: Workspace
    var selected: Bool = false
    var agent: String? = nil
    var prs: [WorkspacePR] = []
    var severity: String = "ok"
    var prsLoading = false
    var pullOn = false
    var pullIntervalSec = 1800
    var pulling = false
    var pulledAt: Date? = nil
    var pullProgress: [AppState.RepoPull] = []
    var jira: JiraIssue? = nil
    var themeMode: ThemeMode = .dark   // in equality so `.equatable()` still re-colors on theme switch
    @State private var showPullDetail = false
    var onOpenPRs: () -> Void = {}
    var onOpenJira: () -> Void = {}
    var onPeekEnter: () -> Void = {}
    var onPeekLeave: () -> Void = {}
    @State private var hovering = false
    @State private var hoverPill = false

    private var dirty: Int { ws.repos.reduce(0) { $0 + $1.dirty } }

    private var orbColor: Color {
        switch agent {
        case "idle":           return Theme.ok
        case "thinking":       return Theme.warn
        case "tool_use":       return Theme.tool
        case "compacting":     return Theme.wsAccent
        case "awaiting_input": return Theme.danger
        default:               return Theme.dim
        }
    }
    private var orbActive: Bool { agent == "thinking" || agent == "tool_use" || agent == "compacting" || agent == "awaiting_input" }

    private var openPRs: [WorkspacePR] { prs.filter { $0.pr != nil } }
    private var prColor: Color {
        switch severity {
        case "danger": return Theme.danger
        case "merged": return Color(hex: 0xa371f7)
        case "warn":   return Theme.warn
        default:       return Theme.ok
        }
    }


    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            AgentOrb(color: orbColor, active: orbActive).padding(.top, 2)
            VStack(alignment: .leading, spacing: 6) {
                Text(ws.title)
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundStyle(ws.isMain ? Theme.accent : Theme.fg)
                    .lineLimit(2).truncationMode(.tail)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(alignment: .top, spacing: 8) {
                    HStack(spacing: 5) {
                        Circle().fill(ws.running == 0 ? Theme.muted : (ws.running >= ws.total ? Theme.ok : Theme.warn))
                            .frame(width: 6, height: 6)
                        if dirty > 0 {
                            Circle().fill(Theme.warn).frame(width: 5, height: 5)
                                .help("\(dirty) repo\(dirty == 1 ? "" : "s") with uncommitted changes")
                        }
                    }
                    Spacer(minLength: 6)
                    VStack(alignment: .trailing, spacing: 3) {
                        if ws.isMain, pullOn || pulling { pullStatus }
                        if !ws.isMain {
                            if !openPRs.isEmpty { prPill.transition(.opacity.combined(with: .scale(scale: 0.8))) }
                            else if prsLoading { Capsule().fill(Theme.dim.opacity(0.12)).frame(width: 24, height: 14) }
                        }
                        if let j = jira, !j.status.isEmpty { jiraChip(j) }
                    }
                }
            }
        }
        .padding(.vertical, 8).padding(.horizontal, 8)
        .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
        .background(selected ? Theme.sel : (hovering ? Theme.hover : .clear),
                    in: RoundedRectangle(cornerRadius: 8))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }

    // Next wall-clock boundary, cron `*/N` style — computed locally so the countdown
    // never depends on a fresh poll (no stuck 0:00).
    private func nextBoundary(_ now: Date) -> Date { pullNextBoundary(now, pullIntervalSec) }

    private var failedCount: Int { pullProgress.filter { $0.state == "failed" }.count }
    private var updatedCount: Int { pullProgress.filter { $0.state == "updated" || $0.state == "migrating" }.count }

    // The sync status shown on the main row: an at-a-glance answer to "did it fetch,
    // and how fresh is it" — like a git client's "Last fetched Xm ago". The countdown
    // to the next run moves into the popover; a bare timer told the user nothing.
    private func syncStatus(_ now: Date) -> (icon: String, text: String, color: Color, spin: Bool) {
        if pulling { return ("", "syncing…", Theme.dim, true) }
        if failedCount > 0 {
            let rel = pulledAt.map { " · " + relTime($0, now) } ?? ""
            return ("exclamationmark.triangle.fill", "\(failedCount) failed\(rel)", Theme.danger, false)
        }
        guard let at = pulledAt else {
            let secs = max(0, Int(nextBoundary(now).timeIntervalSince(now)))
            return ("clock", String(format: "in %d:%02d", secs / 60, secs % 60), Theme.dim, false)
        }
        if now.timeIntervalSince(at) < 6 && updatedCount > 0 {
            return ("checkmark.circle.fill", "\(updatedCount) updated", Theme.ok, false)
        }
        return ("checkmark", "synced " + relTime(at, now), Theme.dim, false)
    }

    @ViewBuilder private var pullStatus: some View {
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            let s = syncStatus(ctx.date)
            HStack(spacing: 4) {
                if s.spin { Spinner(size: 9, lineWidth: 1.6) }
                else { Image(systemName: s.icon).font(.system(size: 8.5, weight: .bold)).foregroundStyle(s.color) }
                Text(s.text).font(Theme.mono(10)).monospacedDigit().foregroundStyle(s.color)
            }
            .transition(.opacity)
            .animation(.easeInOut(duration: 0.35), value: s.text)
        }
        .padding(.horizontal, 5).padding(.vertical, 2)
        .background(showPullDetail ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 5))
        .contentShape(Rectangle())
        .help("Keep main fresh — click for per-repo status")
        .highPriorityGesture(TapGesture().onEnded { showPullDetail.toggle() })
        .popover(isPresented: $showPullDetail, arrowEdge: .bottom) {
            PullDetail(repos: pullProgress, intervalSec: pullIntervalSec, lastPull: pulledAt)
        }
    }

    private func jiraChip(_ j: JiraIssue) -> some View {
        Button(action: onOpenJira) {
            HStack(spacing: 4) {
                Circle().fill(j.color).frame(width: 5, height: 5)
                Text(j.status).font(.system(size: 9.5, weight: .medium)).foregroundStyle(j.color)
                    .lineLimit(1)
            }
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(j.color.opacity(0.12), in: Capsule())
            .contentShape(Capsule())
            .fixedSize()
        }
        .buttonStyle(.plain)
        .help("\(j.key) · \(j.status)\(j.summary.isEmpty ? "" : " — \(j.summary)")")
    }

    private var prPill: some View {
        Button(action: { onOpenPRs() }) {
            HStack(spacing: 4) {
                Image(systemName: "arrow.triangle.pull").font(.system(size: 9))
                Text("\(openPRs.count)").font(Theme.mono(10.5, .semibold))
            }
            .foregroundStyle(prColor)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(prColor.opacity(0.16), in: Capsule())
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        // Only the hovered row publishes its anchor; a .bounds anchor on every row stutters scroll.
        .anchorPreference(key: PRPeekAnchorKey.self, value: .bounds) { hoverPill ? [ws.id: $0] : [:] }
        .onHover { h in
            hoverPill = h
            if h { DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { if hoverPill { onPeekEnter() } } }
            else { onPeekLeave() }
        }
    }

}

struct PRPopRow: View {
    @EnvironmentObject var theme: ThemeManager
    let item: WorkspacePR
    let pr: PRInfo

    private var ciColor: Color {
        switch pr.checks { case .fail: return Theme.danger; case .pending: return Theme.warn; case .pass: return Theme.ok; case .none: return Theme.dim }
    }
    private var statusLine: String {
        var parts: [String] = []
        switch pr.checks { case .fail: parts.append("checks failing"); case .pending: parts.append("checks running"); case .pass: parts.append("all checks passed"); case .none: break }
        switch pr.review { case .approved: parts.append("approved"); case .changes: parts.append("changes requested"); case .review: parts.append("awaiting review"); case .none: break }
        if pr.conflict { parts.insert("merge conflict", at: 0) }
        return parts.isEmpty ? (pr.isDraft ? "draft" : "open") : parts.joined(separator: " · ")
    }

    var body: some View {
        Button { if let u = URL(string: pr.url) { NSWorkspace.shared.open(u) } } label: {
            VStack(alignment: .leading, spacing: 5) {
                Text(pr.title).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.fg).lineLimit(1)
                HStack(spacing: 6) {
                    Text(item.alias).font(Theme.mono(10.5)).foregroundStyle(Theme.accent)
                    if let a = pr.author?.login { Text("@\(a)").font(.system(size: 10.5)).foregroundStyle(Theme.fgMuted) }
                    Text(verbatim: "#\(pr.number)").font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
                    Spacer()
                    if item.behind > 0 {
                        Text("↓\(item.behind) behind").font(.system(size: 10, weight: .medium)).foregroundStyle(Theme.warn)
                            .padding(.horizontal, 5).padding(.vertical, 1).background(Theme.warn.opacity(0.15), in: Capsule())
                    }
                }
                RoundedRectangle(cornerRadius: 2).fill(ciColor).frame(height: 3)
                Text(statusLine).font(.system(size: 10.5)).foregroundStyle(pr.checks == .fail ? Theme.danger : Theme.fgMuted)
            }
            .padding(.horizontal, 8).padding(.vertical, 7)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

private struct PullTick: View {
    @State private var draw: CGFloat = 0
    var body: some View {
        ZStack {
            Circle().fill(Theme.ok)
            CheckShape().trim(from: 0, to: draw)
                .stroke(Color.white, style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round))
                .padding(2.6)
        }
        .frame(width: 11, height: 11)
        .onAppear { withAnimation(.easeOut(duration: 0.3)) { draw = 1 } }
    }
}

private struct CheckShape: Shape {
    func path(in r: CGRect) -> Path {
        var p = Path()
        p.move(to: CGPoint(x: r.minX, y: r.midY))
        p.addLine(to: CGPoint(x: r.minX + r.width * 0.38, y: r.maxY))
        p.addLine(to: CGPoint(x: r.maxX, y: r.minY))
        return p
    }
}

private func relTime(_ d: Date, _ now: Date) -> String {
    let s = Int(now.timeIntervalSince(d))
    if s < 5 { return "just now" }
    if s < 60 { return "\(s)s ago" }
    if s < 3600 { return "\(s / 60)m ago" }
    return "\(s / 3600)h ago"
}

private func pullNextBoundary(_ now: Date, _ intervalSec: Int) -> Date {
    let n = max(1, intervalSec / 60)
    let cal = Calendar.current
    let hourStart = cal.dateInterval(of: .hour, for: now)?.start ?? now
    let minsPast = Int(now.timeIntervalSince(hourStart) / 60)
    let k = minsPast / n + 1
    let mins = k * n >= 60 ? 60 : k * n
    return hourStart.addingTimeInterval(TimeInterval(mins * 60))
}

private struct PullDetail: View {
    let repos: [AppState.RepoPull]
    var intervalSec: Int = 1800
    var lastPull: Date? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            header
                .padding(.horizontal, 8).padding(.top, 6).padding(.bottom, 4)
            Divider().overlay(Theme.borderSoft)
            if repos.isEmpty {
                Text("No sync yet — waiting for the first run.").font(.system(size: 11)).foregroundStyle(Theme.dim)
                    .padding(.horizontal, 8).padding(.vertical, 8)
            }
            ForEach(repos) { r in
                HStack(spacing: 8) {
                    icon(r.state).frame(width: 12)
                    Text(r.repo).font(Theme.mono(11.5)).foregroundStyle(Theme.fg).lineLimit(1)
                    Spacer(minLength: 10)
                    Text(label(r.state)).font(Theme.mono(10)).foregroundStyle(r.state == "failed" ? Theme.danger : Theme.dim)
                }
                .padding(.horizontal, 8).padding(.vertical, 4)
                .contentShape(Rectangle())
                .help(r.state == "failed" && !r.detail.isEmpty ? r.detail : label(r.state))
            }
        }
        .padding(4).frame(width: 250)
        .background(Theme.panel3)
    }

    private var header: some View {
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            let secs = max(0, Int(pullNextBoundary(ctx.date, intervalSec).timeIntervalSince(ctx.date)))
            let failed = repos.filter { $0.state == "failed" }.count
            HStack(spacing: 6) {
                Text("Keep main fresh").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.fgMuted)
                Spacer()
                if failed > 0 {
                    Text("\(failed) failed").font(Theme.mono(9.5)).foregroundStyle(Theme.danger)
                } else if let d = lastPull {
                    Text(relTime(d, ctx.date)).font(Theme.mono(9.5)).foregroundStyle(Theme.dim)
                }
                Text(String(format: "next %d:%02d", secs / 60, secs % 60)).font(Theme.mono(9.5)).monospacedDigit().foregroundStyle(Theme.dim)
            }
        }
    }

    @ViewBuilder private func icon(_ s: String) -> some View {
        switch s {
        case "pulling", "migrating": Spinner(size: 9, lineWidth: 1.6)
        case "updated": PullTick()
        case "failed": Image(systemName: "xmark.circle.fill").font(.system(size: 10)).foregroundStyle(Theme.danger)
        case "skipped": Image(systemName: "minus.circle").font(.system(size: 10)).foregroundStyle(Theme.dim)
        case "nochange": Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.dim)
        default: Circle().stroke(Theme.dim.opacity(0.4), lineWidth: 1).frame(width: 8, height: 8)
        }
    }

    private func label(_ s: String) -> String {
        switch s {
        case "pulling": return "pulling"
        case "migrating": return "migrating"
        case "updated": return "updated"
        case "nochange": return "up to date"
        case "skipped": return "local changes"
        case "failed": return "failed"
        default: return "waiting"
        }
    }
}
