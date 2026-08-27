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

struct WsCard: View {
    @EnvironmentObject var theme: ThemeManager
    let ws: Workspace
    var selected: Bool = false
    var agent: String? = nil
    var prs: [WorkspacePR] = []
    var severity: String = "ok"
    var prsLoading = false
    var jira: JiraIssue? = nil
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
                        Circle().fill(ws.running > 0 ? Theme.ok : Theme.muted).frame(width: 6, height: 6)
                        (Text("\(ws.running)").foregroundStyle(Theme.fgMuted)
                            + Text("/\(ws.total)").foregroundStyle(Theme.dim))
                            .font(Theme.mono(11))
                        if dirty > 0 {
                            Circle().fill(Theme.warn).frame(width: 5, height: 5)
                                .help("\(dirty) repo\(dirty == 1 ? "" : "s") with uncommitted changes")
                        }
                    }
                    Spacer(minLength: 6)
                    VStack(alignment: .trailing, spacing: 3) {
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
