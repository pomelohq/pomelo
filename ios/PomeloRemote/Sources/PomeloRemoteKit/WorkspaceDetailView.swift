import SwiftUI
import UIKit

struct WorkspaceDetailView: View {
    @EnvironmentObject var theme: ThemeManager
    let client: RemoteClient
    let workspace: WorkspaceRow
    @State private var tab: Tab = .agent

    enum Tab: String, CaseIterable { case agent = "Agent", jira = "Jira", prs = "PRs" }

    private var tabs: [Tab] { workspace.isMain ? [.agent] : Tab.allCases }

    var body: some View {
        VStack(spacing: 0) {
            if tabs.count > 1 {
                Picker("", selection: $tab) {
                    ForEach(tabs, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(Theme.bgSoft)
            }

            switch tab {
            case .agent: AgentView(client: client, workspace: workspace)
            case .jira: JiraTab(client: client, workspace: workspace)
            case .prs: PRsTab(client: client, workspace: workspace)
            }
        }
        .background(Theme.bg.ignoresSafeArea())
        .navigationTitle(workspace.title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(Theme.bgSoft, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbarColorScheme(.dark, for: .navigationBar)
    }
}

struct MarkdownBlock: View {
    let text: String

    private enum Block: Identifiable {
        case heading(Int, String), paragraph(String), bullet(String), code(String)
        var id: String { UUID().uuidString }
    }

    private var blocks: [Block] {
        var out: [Block] = []
        let lines = text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        var i = 0
        while i < lines.count {
            let line = lines[i]
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") {
                i += 1
                var code: [String] = []
                while i < lines.count, !lines[i].trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                    code.append(lines[i]); i += 1
                }
                i += 1
                out.append(.code(code.joined(separator: "\n")))
                continue
            }
            if let h = heading(t) { out.append(.heading(h.0, h.1)); i += 1; continue }
            if let b = bullet(t) { out.append(.bullet(b)); i += 1; continue }
            if t.isEmpty { i += 1; continue }
            out.append(.paragraph(line)); i += 1
        }
        return out
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(blocks) { view($0) }
        }
    }

    @ViewBuilder private func view(_ b: Block) -> some View {
        switch b {
        case .heading(let n, let s):
            Text(inline(s))
                .font(Theme.ui(n <= 1 ? 16 : n == 2 ? 15 : n == 3 ? 13.5 : 12.5, n >= 3 ? .semibold : .bold))
                .foregroundStyle(Theme.fg).padding(.top, 3)
        case .paragraph(let s):
            Text(inline(s)).font(Theme.ui(13)).foregroundStyle(Theme.fgSoft)
                .fixedSize(horizontal: false, vertical: true)
        case .bullet(let s):
            HStack(alignment: .top, spacing: 6) {
                Text("•").foregroundStyle(Theme.fgMuted)
                Text(inline(s)).foregroundStyle(Theme.fgSoft).fixedSize(horizontal: false, vertical: true)
            }.font(Theme.ui(13))
        case .code(let s):
            ScrollView(.horizontal, showsIndicators: true) {
                Text(s).font(Theme.mono(11.5)).foregroundStyle(Theme.fg)
                    .textSelection(.enabled).fixedSize(horizontal: true, vertical: false)
                    .padding(10)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
        }
    }

    private func heading(_ t: String) -> (Int, String)? {
        guard t.hasPrefix("#") else { return nil }
        var n = 0
        for c in t { if c == "#" { n += 1 } else { break } }
        guard n <= 6, t.count > n, t[t.index(t.startIndex, offsetBy: n)] == " " else { return nil }
        return (n, String(t.dropFirst(n + 1)))
    }
    private func bullet(_ l: String) -> String? {
        for p in ["- ", "* ", "+ ", "• "] where l.hasPrefix(p) { return String(l.dropFirst(p.count)) }
        return nil
    }
    private func inline(_ s: String) -> AttributedString {
        guard var a = try? AttributedString(markdown: s, options: .init(
            allowsExtendedAttributes: true,
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible)) else { return AttributedString(s) }
        for run in a.runs where run.inlinePresentationIntent?.contains(.code) == true {
            a[run.range].font = .system(size: 12, weight: .regular, design: .monospaced)
            a[run.range].backgroundColor = Theme.chip
            a[run.range].foregroundColor = Theme.fg
        }
        return a
    }
}

private struct JiraFull: Decodable {
    var key = "", summary = "", status = "", category = "", description = "", url = ""
    var comments: [Comment] = []
    var webLinks: [WebLink] = []
    struct Comment: Decodable, Identifiable {
        var id = "", author = "", avatar = "", created = "", body = ""
        var whenLong: String { JiraDates.long(created) ?? String(created.replacingOccurrences(of: "T", with: " ").prefix(16)) }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            id = try c.decodeIfPresent(String.self, forKey: .id) ?? UUID().uuidString
            author = try c.decodeIfPresent(String.self, forKey: .author) ?? ""
            avatar = try c.decodeIfPresent(String.self, forKey: .avatar) ?? ""
            created = try c.decodeIfPresent(String.self, forKey: .created) ?? ""
            body = try c.decodeIfPresent(String.self, forKey: .body) ?? ""
        }
        enum K: String, CodingKey { case id, author, avatar, created, body }
    }
    struct WebLink: Decodable, Identifiable {
        var title = "", url = ""
        var id: String { url }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
            url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        }
        enum K: String, CodingKey { case title, url }
    }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        category = try c.decodeIfPresent(String.self, forKey: .category) ?? ""
        description = try c.decodeIfPresent(String.self, forKey: .description) ?? ""
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        comments = try c.decodeIfPresent([Comment].self, forKey: .comments) ?? []
        webLinks = try c.decodeIfPresent([WebLink].self, forKey: .web_links) ?? []
    }
    enum K: String, CodingKey { case key, summary, status, category, description, url, comments, web_links }
}

private struct JiraTab: View {
    let client: RemoteClient
    let workspace: WorkspaceRow
    @State private var issue: JiraFull?
    @State private var loading = true

    var body: some View {
        ScrollView {
            if loading {
                ProgressView().tint(Theme.accent).padding(40)
            } else if let j = issue, !j.key.isEmpty {
                VStack(alignment: .leading, spacing: 16) {
                    HStack(spacing: 8) {
                        Text(j.key).font(Theme.mono(13, .semibold)).foregroundStyle(Theme.accent)
                        chip(j.status, jiraColor(j.category))
                        Spacer()
                        if let url = URL(string: j.url), !j.url.isEmpty {
                            Link(destination: url) { Image(systemName: "arrow.up.forward.square").foregroundStyle(Theme.accent) }
                        }
                    }
                    Text(j.summary).font(Theme.ui(18, .bold)).foregroundStyle(Theme.fg)

                    if !j.description.isEmpty {
                        MarkdownBlock(text: j.description).textSelection(.enabled)
                    }
                    if !j.webLinks.isEmpty {
                        VStack(alignment: .leading, spacing: 6) {
                            ForEach(j.webLinks) { l in
                                if let u = URL(string: l.url) {
                                    Link(destination: u) {
                                        Label(l.title.isEmpty ? l.url : l.title, systemImage: "link")
                                            .font(Theme.ui(12)).foregroundStyle(Theme.accent)
                                    }
                                }
                            }
                        }
                    }
                    if !j.comments.isEmpty {
                        Divider().overlay(Theme.borderSoft)
                        Text("Comments (\(j.comments.count))").font(Theme.ui(13, .semibold)).foregroundStyle(Theme.fgMuted)
                        JiraCommentsView(comments: j.comments)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
            } else {
                empty("ticket", "No Jira ticket linked to this branch")
            }
        }
        .background(Theme.bg)
        .task { await load(force: false) }
        .refreshable { await load(force: true) }
    }

    private func load(force: Bool) async {
        guard let key = jiraKey(workspace.branch) else { loading = false; return }
        if let d = try? await client.jiraIssue(key: key, force: force) { issue = PomJSON.decode(JiraFull.self, from: d) }
        loading = false
    }

    private func chip(_ t: String, _ c: Color) -> some View {
        Text(t).font(Theme.ui(11, .medium)).foregroundStyle(c)
            .padding(.horizontal, 7).padding(.vertical, 2).background(c.opacity(0.14), in: Capsule())
    }

    private func jiraColor(_ cat: String) -> Color {
        switch cat { case "done": return Theme.ok; case "indeterminate": return Theme.warn; default: return Theme.fgMuted }
    }
}

enum JiraDates {
    private static let parse: DateFormatter = {
        let f = DateFormatter(); f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd'T'HH:mm:ss.SSSZ"; return f
    }()
    private static let out: DateFormatter = {
        let f = DateFormatter(); f.locale = Locale(identifier: "en_US")
        f.dateFormat = "MMMM d, yyyy 'at' h:mm a"; return f
    }()
    static func long(_ iso: String) -> String? {
        guard let d = parse.date(from: iso) else { return nil }
        return out.string(from: d)
    }
}

struct AvatarView: View {
    let url: String
    let name: String
    let size: CGFloat
    private var initials: String {
        let parts = name.split(separator: " ").prefix(2)
        let s = parts.map { String($0.prefix(1)) }.joined()
        return s.isEmpty ? "?" : s.uppercased()
    }
    var body: some View {
        Group {
            if let u = URL(string: url), !url.isEmpty {
                AsyncImage(url: u) { phase in
                    if let img = phase.image { img.resizable().scaledToFill() }
                    else { fallback }
                }
            } else { fallback }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
    }
    private var fallback: some View {
        Circle().fill(Theme.panel3)
            .overlay(Text(initials).font(Theme.ui(size * 0.4, .semibold)).foregroundStyle(Theme.fgMuted))
    }
}

private struct JiraCommentsView: View {
    let comments: [JiraFull.Comment]
    @State private var collapsed: Set<String> = []

    private struct Row { let c: JiraFull.Comment; let parent: String?; var replies = 0; var last = false }

    private func threaded(_ all: [JiraFull.Comment]) -> [Row] {
        func mentioned(_ body: String) -> String? {
            let t = body.trimmingCharacters(in: .whitespacesAndNewlines)
            guard t.hasPrefix("**@"), let end = t.range(of: "**", range: t.index(t.startIndex, offsetBy: 3)..<t.endIndex)
            else { return nil }
            return String(t[t.index(t.startIndex, offsetBy: 3)..<end.lowerBound])
        }
        let chrono = all.sorted { $0.created < $1.created }
        var children: [String: [JiraFull.Comment]] = [:]
        var parentOf: [String: String] = [:]
        for c in chrono {
            guard let name = mentioned(c.body),
                  let parent = chrono.last(where: { $0.created < c.created && $0.author == name })
            else { continue }
            children[parent.id, default: []].append(c)
            parentOf[c.id] = parent.id
        }
        var out: [Row] = []
        for top in chrono.filter({ parentOf[$0.id] == nil }).sorted(by: { $0.created > $1.created }) {
            let kids = children[top.id] ?? []
            out.append(Row(c: top, parent: nil, replies: kids.count))
            for (i, r) in kids.enumerated() { out.append(Row(c: r, parent: top.id, last: i == kids.count - 1)) }
        }
        return out
    }

    private var rows: [Row] {
        threaded(comments).filter { $0.parent == nil || !collapsed.contains($0.parent!) }
    }

    private let avatarSize: CGFloat = 32
    private let replyAvatar: CGFloat = 24
    private let railX: CGFloat = 0 + 16

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(rows, id: \.c.id) { row in
                commentRow(row)
                if row.parent == nil, !(row.replies > 0 && !collapsed.contains(row.c.id)) {
                    Divider().overlay(Theme.borderSoft)
                } else if row.last {
                    Divider().overlay(Theme.borderSoft)
                }
            }
        }
    }

    private func commentRow(_ row: Row) -> some View {
        let c = row.c
        let isReply = row.parent != nil
        let collapsedNow = collapsed.contains(c.id)
        let showsRail = (!isReply && row.replies > 0 && !collapsedNow) || isReply
        return HStack(alignment: .top, spacing: 10) {
            if isReply { Color.clear.frame(width: railX + 8) }
            AvatarView(url: c.avatar, name: c.author, size: isReply ? replyAvatar : avatarSize)
                .padding(.top, isReply ? 2 : 0)
            VStack(alignment: .leading, spacing: 3) {
                Text(c.author.isEmpty ? "—" : c.author).font(Theme.ui(12.5, .semibold)).foregroundStyle(Theme.fg).lineLimit(1)
                Text(c.whenLong).font(Theme.ui(10.5)).foregroundStyle(Theme.fgMuted)
                MarkdownBlock(text: c.body).textSelection(.enabled).padding(.top, 3)
                if !isReply, row.replies > 0 {
                    Button {
                        if collapsedNow { collapsed.remove(c.id) } else { collapsed.insert(c.id) }
                    } label: {
                        HStack(spacing: 4) {
                            Image(systemName: collapsedNow ? "chevron.right" : "chevron.down").font(.system(size: 9, weight: .semibold))
                            Text("\(row.replies) \(row.replies == 1 ? "reply" : "replies")").font(Theme.ui(11.5, .medium))
                        }.foregroundStyle(Theme.accent)
                    }.buttonStyle(.plain).padding(.top, 4)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.trailing, 4)
        .padding(.top, isReply ? 6 : 16).padding(.bottom, isReply && !row.last ? 6 : 16)
        .background(alignment: .topLeading) {
            if showsRail { threadRail(isReply: isReply, last: row.last) }
        }
    }

    private func threadRail(isReply: Bool, last: Bool) -> some View {
        GeometryReader { geo in
            Path { p in
                let x = railX
                if !isReply {
                    p.move(to: CGPoint(x: x, y: 16 + avatarSize + 4))
                    p.addLine(to: CGPoint(x: x, y: geo.size.height))
                } else {
                    let cy = 6 + 2 + replyAvatar / 2
                    p.move(to: CGPoint(x: x, y: 0))
                    p.addLine(to: CGPoint(x: x, y: cy - 10))
                    p.addQuadCurve(to: CGPoint(x: x + 10, y: cy), control: CGPoint(x: x, y: cy))
                    p.addLine(to: CGPoint(x: railX + 8 + 10 - 4, y: cy))
                    if !last {
                        p.move(to: CGPoint(x: x, y: cy - 10))
                        p.addLine(to: CGPoint(x: x, y: geo.size.height))
                    }
                }
            }
            .stroke(Theme.border, lineWidth: 1.5)
        }
    }
}

private struct PRPayload: Decodable { var prs: [WsPR] = [] }

private struct PRDetailPayload: Decodable { var pr: PRInfo? }

struct FlowLayout: Layout {
    var spacing: CGFloat = 6
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxW = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, rowH: CGFloat = 0
        for s in subviews {
            let sz = s.sizeThatFits(.unspecified)
            if x + sz.width > maxW, x > 0 { x = 0; y += rowH + spacing; rowH = 0 }
            x += sz.width + spacing
            rowH = max(rowH, sz.height)
        }
        return CGSize(width: maxW == .infinity ? x : maxW, height: y + rowH)
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, rowH: CGFloat = 0
        for s in subviews {
            let sz = s.sizeThatFits(.unspecified)
            if x + sz.width > bounds.maxX, x > bounds.minX { x = bounds.minX; y += rowH + spacing; rowH = 0 }
            s.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(sz))
            x += sz.width + spacing
            rowH = max(rowH, sz.height)
        }
    }
}

private struct WsPR: Decodable, Identifiable {
    var repo = "", pr: PRInfo?
    var id: String { repo }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
        pr = try c.decodeIfPresent(PRInfo.self, forKey: .pr)
    }
    enum K: String, CodingKey { case repo, pr }
}

struct PRInfo: Decodable {
    var number = 0, title = "", state = "", url = "", body = ""
    var headRefName = "", baseRefName = "", authorLogin = ""
    var isDraft = false, additions = 0, deletions = 0, changedFiles = 0
    var checks = ""
    var reviewers: [Reviewer] = []
    var checkRuns: [Check] = []
    var labels: [Label] = []

    struct Label: Decodable, Identifiable {
        var name = "", color = ""
        var id: String { name }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
            color = try c.decodeIfPresent(String.self, forKey: .color) ?? ""
        }
        enum K: String, CodingKey { case name, color }
    }

    struct Reviewer: Decodable, Identifiable {
        var name = "", state = ""
        var id: String { name }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
            state = try c.decodeIfPresent(String.self, forKey: .state) ?? ""
        }
        enum K: String, CodingKey { case name, state }
    }
    struct Check: Decodable, Identifiable {
        var name = "", workflowName = "", conclusion = "", status = ""
        var id = UUID().uuidString
        var label: String { workflowName.isEmpty ? name : workflowName }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
            workflowName = try c.decodeIfPresent(String.self, forKey: .workflowName) ?? ""
            conclusion = try c.decodeIfPresent(String.self, forKey: .conclusion) ?? ""
            status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        }
        enum K: String, CodingKey { case name, workflowName, conclusion, status }
    }
    struct Author: Decodable {
        var login = ""
        init(from d: Decoder) throws {
            login = try d.container(keyedBy: K.self).decodeIfPresent(String.self, forKey: .login) ?? ""
        }
        enum K: String, CodingKey { case login }
    }

    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        number = try c.decodeIfPresent(Int.self, forKey: .number) ?? 0
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? ""
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        body = try c.decodeIfPresent(String.self, forKey: .body) ?? ""
        headRefName = try c.decodeIfPresent(String.self, forKey: .headRefName) ?? ""
        baseRefName = try c.decodeIfPresent(String.self, forKey: .baseRefName) ?? ""
        authorLogin = (try c.decodeIfPresent(Author.self, forKey: .author))?.login ?? ""
        isDraft = try c.decodeIfPresent(Bool.self, forKey: .isDraft) ?? false
        additions = try c.decodeIfPresent(Int.self, forKey: .additions) ?? 0
        deletions = try c.decodeIfPresent(Int.self, forKey: .deletions) ?? 0
        changedFiles = try c.decodeIfPresent(Int.self, forKey: .changedFiles) ?? 0
        reviewers = try c.decodeIfPresent([Reviewer].self, forKey: .reviewers) ?? []
        labels = try c.decodeIfPresent([Label].self, forKey: .labels) ?? []
        checkRuns = try c.decodeIfPresent([Check].self, forKey: .statusCheckRollup) ?? []
        let concs = checkRuns.map { $0.conclusion.uppercased() }
        if concs.contains("FAILURE") || concs.contains("ERROR") { checks = "failing" }
        else if concs.contains(where: { $0 == "PENDING" || $0 == "IN_PROGRESS" || $0.isEmpty }) && !concs.isEmpty { checks = "pending" }
        else if !concs.isEmpty { checks = "passing" }
    }
    enum K: String, CodingKey { case number, title, state, url, body, headRefName, baseRefName, author, isDraft, additions, deletions, changedFiles, statusCheckRollup, reviewers, labels }
}

private struct PRsTab: View {
    let client: RemoteClient
    let workspace: WorkspaceRow
    @State private var prs: [WsPR] = []
    @State private var loading = true

    private var withPR: [WsPR] { prs.filter { $0.pr != nil } }

    var body: some View {
        ScrollView {
            if loading {
                ProgressView().tint(Theme.accent).padding(40)
            } else if withPR.isEmpty {
                empty("arrow.triangle.pull", "No pull requests")
            } else {
                VStack(spacing: 8) {
                    ForEach(withPR) { item in
                        if let pr = item.pr {
                            NavigationLink { PRDetailView(client: client, branch: workspace.branch, isMain: workspace.isMain, repo: item.repo, pr: pr) } label: { prCard(item.repo, pr) }
                                .buttonStyle(.plain)
                        }
                    }
                }
                .padding(12)
            }
        }
        .background(Theme.bg)
        .task { await load(force: false) }
        .refreshable { await load(force: true) }
    }

    private func load(force: Bool) async {
        if force { await client.prRefresh() }
        if let d = try? await client.prWorkspace(branch: workspace.branch, isMain: workspace.isMain) {
            prs = PomJSON.decode(PRPayload.self, from: d)?.prs ?? []
        }
        loading = false
    }

    private func prCard(_ repo: String, _ pr: PRInfo) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text(repo).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted)
                Text("#\(String(pr.number))").font(Theme.mono(11, .semibold)).foregroundStyle(Theme.accent)
                badge(pr.state.lowercased(), stateColor(pr.state))
                if pr.isDraft { badge("draft", Theme.fgMuted) }
                if !pr.checks.isEmpty { badge(pr.checks, checkColor(pr.checks)) }
                Spacer()
            }
            Text(pr.title).font(Theme.ui(14, .semibold)).foregroundStyle(Theme.fg)
            HStack(spacing: 10) {
                Text("+\(pr.additions)").font(Theme.mono(11)).foregroundStyle(Theme.ok)
                Text("-\(pr.deletions)").font(Theme.mono(11)).foregroundStyle(Theme.danger)
                Text("\(pr.changedFiles) files").font(Theme.mono(11)).foregroundStyle(Theme.fgMuted)
                Spacer()
                if let url = URL(string: pr.url), !pr.url.isEmpty {
                    Link(destination: url) { Image(systemName: "arrow.up.forward.square").foregroundStyle(Theme.accent) }
                }
            }
        }
        .padding(12)
        .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderSoft, lineWidth: 1))
    }

    private func badge(_ t: String, _ c: Color) -> some View {
        Text(t).font(Theme.ui(9, .medium)).foregroundStyle(c)
            .padding(.horizontal, 5).padding(.vertical, 1).background(c.opacity(0.14), in: Capsule())
    }

    private func stateColor(_ s: String) -> Color {
        switch s.uppercased() {
        case "MERGED": return Color(hex: 0xa371f7)
        case "CLOSED": return Theme.danger
        case "OPEN": return Theme.ok
        default: return Theme.fgMuted
        }
    }

    private func checkColor(_ s: String) -> Color {
        switch s { case "passing": return Theme.ok; case "failing": return Theme.danger; default: return Theme.warn }
    }
}

private struct PRDetailView: View {
    let client: RemoteClient
    let branch: String
    let isMain: Bool
    let repo: String
    let pr: PRInfo
    @State private var detail: PRInfo?

    private var full: PRInfo { detail ?? pr }

    @State private var tab: Tab = .overview
    enum Tab: String, CaseIterable { case overview = "Overview", files = "Files", conversation = "Conversation" }

    var body: some View {
        VStack(spacing: 0) {
            header
            Picker("", selection: $tab) {
                ForEach(Tab.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16).padding(.vertical, 8)
            .background(Theme.bgSoft)
            Rectangle().fill(Theme.borderSoft).frame(height: 1)

            switch tab {
            case .overview: overview
            case .files: PRFilesTab(client: client, branch: branch, isMain: isMain, repo: repo)
            case .conversation: PRConversationTab(client: client, branch: branch, isMain: isMain, repo: repo)
            }
        }
        .background(Theme.bg.ignoresSafeArea())
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                Text("\(repo)  #\(String(pr.number))").font(Theme.mono(12, .semibold)).foregroundStyle(Theme.accent)
            }
        }
        .toolbarBackground(Theme.bgSoft, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbarColorScheme(.dark, for: .navigationBar)
    }

    private var header: some View {
        let p = full
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                StatusPill(text: p.state, color: stateColor(p.state), uppercase: true)
                if p.isDraft { StatusPill(text: "draft", color: Theme.fgMuted, uppercase: true) }
                Spacer()
                Text("+\(p.additions)").font(Theme.mono(11)).foregroundStyle(Theme.ok)
                Text("-\(p.deletions)").font(Theme.mono(11)).foregroundStyle(Theme.danger)
                if let u = URL(string: p.url), !p.url.isEmpty {
                    Link(destination: u) { Image(systemName: "arrow.up.forward.square").font(.system(size: 13)).foregroundStyle(Theme.accent) }
                }
            }
            Text(p.title).font(Theme.ui(17, .bold)).foregroundStyle(Theme.fg)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 6) {
                AvatarView(url: "", name: p.authorLogin, size: 16)
                Text(p.authorLogin).font(Theme.ui(11)).foregroundStyle(Theme.fgMuted)
                Text("\(p.headRefName) → \(p.baseRefName)").font(Theme.mono(10)).foregroundStyle(Theme.dim)
                    .lineLimit(1).truncationMode(.middle)
            }
        }
        .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.bgSoft)
    }

    private var overview: some View {
        let p = full
        return ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if !p.reviewers.isEmpty { reviewersSection(p.reviewers) }
                if !p.labels.isEmpty { labelChips(p.labels) }
                if !p.reviewers.isEmpty || !p.labels.isEmpty {
                    Rectangle().fill(Theme.borderSoft).frame(height: 1)
                }
                if !p.body.isEmpty {
                    descriptionCard(author: p.authorLogin, body: p.body)
                } else {
                    Text("No description.").font(Theme.ui(12)).foregroundStyle(Theme.dim)
                }
                if !p.checkRuns.isEmpty {
                    Rectangle().fill(Theme.borderSoft).frame(height: 1)
                    checksSection(p.checkRuns)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(16)
        }
        .task { await loadDetail(force: false) }
        .refreshable { await loadDetail(force: true) }
    }

    private func reviewersSection(_ list: [PRInfo.Reviewer]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Reviewers")
            ForEach(list) { r in
                HStack(spacing: 8) {
                    Image(systemName: reviewerIcon(r.state)).font(.system(size: 12)).foregroundStyle(reviewColor(r.state)).frame(width: 16)
                    Text(r.name).font(Theme.ui(12.5)).foregroundStyle(Theme.fg)
                    Spacer()
                    StatusPill(text: r.state, color: reviewColor(r.state), uppercase: true)
                }
            }
        }
    }

    private func checksSection(_ checks: [PRInfo.Check]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Checks")
            ForEach(checks) { ck in
                HStack(spacing: 8) {
                    Image(systemName: checkIcon(ck.conclusion)).font(.system(size: 12)).foregroundStyle(checkRunColor(ck.conclusion)).frame(width: 16)
                    Text(ck.label).font(Theme.ui(12.5)).foregroundStyle(Theme.fg).lineLimit(1)
                    Spacer()
                    Text(ck.conclusion.lowercased()).font(Theme.mono(10)).foregroundStyle(checkRunColor(ck.conclusion))
                }
            }
        }
    }

    private func reviewerIcon(_ s: String) -> String {
        switch s.lowercased() {
        case let x where x.contains("approv"): return "checkmark.seal.fill"
        case let x where x.contains("change"): return "exclamationmark.triangle.fill"
        case let x where x.contains("comment"): return "text.bubble.fill"
        default: return "clock.fill"
        }
    }

    private func loadDetail(force: Bool) async {
        if !force && detail != nil { return }
        if force { await client.prRefresh() }
        if let d = try? await client.prDetail(branch: branch, repo: repo, isMain: isMain) {
            detail = PomJSON.decode(PRDetailPayload.self, from: d)?.pr
        }
    }

    private func labelChips(_ labels: [PRInfo.Label]) -> some View {
        FlowLayout(spacing: 6) {
            ForEach(labels) { l in
                let c = labelColor(l.color)
                Text(l.name).font(Theme.ui(10.5, .medium)).foregroundStyle(c)
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(c.opacity(0.14), in: Capsule())
                    .overlay(Capsule().stroke(c.opacity(0.4), lineWidth: 1))
            }
        }
    }

    private func descriptionCard(author: String, body: String) -> some View {
        Card(cornerRadius: 8) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    AvatarView(url: "", name: author, size: 18)
                    Text(author.isEmpty ? "author" : author).font(Theme.ui(11.5, .semibold)).foregroundStyle(Theme.fg)
                    Text("opened this pull request").font(Theme.ui(11.5)).foregroundStyle(Theme.fgMuted)
                    Spacer()
                }
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(Theme.panel3)
                Rectangle().fill(Theme.borderSoft).frame(height: 1)
                MarkdownBlock(text: body).textSelection(.enabled).padding(12)
            }
        }
    }

    private func labelColor(_ hex: String) -> Color {
        guard let v = UInt32(hex.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16) else { return Theme.fgMuted }
        return Color(hex: v)
    }

    private func badge(_ t: String, _ c: Color) -> some View {
        Text(t).font(Theme.ui(9, .medium)).foregroundStyle(c)
            .padding(.horizontal, 5).padding(.vertical, 1).background(c.opacity(0.14), in: Capsule())
    }
    private func stateColor(_ s: String) -> Color {
        switch s.uppercased() { case "MERGED": return Color(hex: 0xa371f7); case "CLOSED": return Theme.danger; case "OPEN": return Theme.ok; default: return Theme.fgMuted }
    }
    private func reviewColor(_ s: String) -> Color {
        switch s.lowercased() {
        case let x where x.contains("approv"): return Theme.ok
        case let x where x.contains("change"): return Theme.danger
        case let x where x.contains("comment"): return Theme.fgMuted
        default: return Theme.warn
        }
    }
    private func checkRunColor(_ s: String) -> Color {
        switch s.uppercased() { case "SUCCESS": return Theme.ok; case "FAILURE", "ERROR": return Theme.danger; case "SKIPPED", "NEUTRAL", "": return Theme.fgMuted; default: return Theme.warn }
    }
    private func checkIcon(_ s: String) -> String {
        switch s.uppercased() { case "SUCCESS": return "checkmark.circle.fill"; case "FAILURE", "ERROR": return "xmark.circle.fill"; case "SKIPPED", "NEUTRAL": return "minus.circle"; default: return "clock" }
    }
}

private struct DiffLine: Identifiable, Decodable {
    enum Kind: String, Decodable { case context, add, del, hunk }
    var id = 0
    var kind: Kind = .context
    var oldN: Int?
    var newN: Int?
    var text = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decodeIfPresent(Int.self, forKey: .id) ?? 0
        kind = (try? c.decodeIfPresent(Kind.self, forKey: .kind) ?? .context) ?? .context
        oldN = try c.decodeIfPresent(Int.self, forKey: .oldN)
        newN = try c.decodeIfPresent(Int.self, forKey: .newN)
        text = try c.decodeIfPresent(String.self, forKey: .text) ?? ""
    }
    enum K: String, CodingKey { case id, kind, text, oldN = "old_n", newN = "new_n" }
}

private struct DiffFile: Identifiable, Decodable {
    var path = ""
    var oldPath: String?
    var status = ""
    var adds = 0
    var dels = 0
    var binary = false
    var lines: [DiffLine] = []
    var id: String { path }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
        oldPath = try c.decodeIfPresent(String.self, forKey: .oldPath)
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        adds = try c.decodeIfPresent(Int.self, forKey: .adds) ?? 0
        dels = try c.decodeIfPresent(Int.self, forKey: .dels) ?? 0
        binary = try c.decodeIfPresent(Bool.self, forKey: .binary) ?? false
        lines = try c.decodeIfPresent([DiffLine].self, forKey: .lines) ?? []
    }
    enum K: String, CodingKey { case path, status, adds, dels, binary, lines, oldPath = "old_path" }
}

enum DiffLang { case ruby, jsts, go, python, swift, generic }

enum DiffSyntax {
    static func lang(for path: String) -> DiffLang {
        switch (path as NSString).pathExtension.lowercased() {
        case "rb", "rake", "ru": return .ruby
        case "js", "jsx", "ts", "tsx", "mjs", "cjs": return .jsts
        case "go": return .go
        case "py": return .python
        case "swift": return .swift
        default:
            if (path as NSString).lastPathComponent.lowercased() == "gemfile" { return .ruby }
            return .generic
        }
    }

    private static func keywords(_ l: DiffLang) -> [String] {
        switch l {
        case .ruby: return ["def","end","class","module","do","if","elsif","else","unless","while","until","return","yield","self","nil","true","false","require","require_relative","attr_accessor","attr_reader","attr_writer","begin","rescue","ensure","raise","then","case","when","new","and","or","not","next","break","super"]
        case .jsts: return ["const","let","var","function","return","if","else","for","while","do","class","new","import","from","export","default","async","await","try","catch","finally","throw","typeof","instanceof","extends","implements","interface","type","enum","public","private","protected","readonly","static","void","null","undefined","true","false","this","super","switch","case","break","continue"]
        case .go: return ["func","package","import","return","if","else","for","range","type","struct","interface","map","chan","go","defer","var","const","nil","true","false","switch","case","break","continue","select","default","fallthrough","goto"]
        case .python: return ["def","class","return","if","elif","else","for","while","import","from","as","try","except","finally","raise","with","lambda","None","True","False","self","and","or","not","in","is","pass","break","continue","yield","global","nonlocal","async","await"]
        case .swift: return ["func","let","var","return","if","else","for","while","class","struct","enum","protocol","extension","import","guard","switch","case","break","continue","private","public","internal","static","nil","true","false","self","init","throws","try","async","await","some","in"]
        case .generic: return ["return","if","else","for","while","class","function","func","import","true","false","null","nil"]
        }
    }
    private static func commentToken(_ l: DiffLang) -> String { (l == .ruby || l == .python) ? "#" : "//" }

    static func nsHighlight(_ text: String, lang: DiffLang, font: UIFont, base: UIColor) -> NSMutableAttributedString {
        let ns = NSMutableAttributedString(string: text.isEmpty ? " " : text, attributes: [.font: font, .foregroundColor: base])
        let full = NSRange(location: 0, length: (ns.string as NSString).length)
        func apply(_ pattern: String, _ color: UIColor) {
            guard let re = try? NSRegularExpression(pattern: pattern) else { return }
            re.enumerateMatches(in: ns.string, range: full) { m, _, _ in
                if let r = m?.range { ns.addAttribute(.foregroundColor, value: color, range: r) }
            }
        }
        apply("\\b[A-Z][A-Za-z0-9_]*\\b", UIColor(Color(hex: 0x61afef)))
        apply("\\b\\d[\\d_]*(\\.\\d+)?\\b", UIColor(Color(hex: 0xd19a66)))
        let kw = keywords(lang)
        if !kw.isEmpty { apply("\\b(" + kw.joined(separator: "|") + ")\\b", UIColor(Color(hex: 0xc678dd))) }
        apply("\"([^\"\\\\]|\\\\.)*\"|'([^'\\\\]|\\\\.)*'", UIColor(Color(hex: 0x98c379)))
        let ct = NSRegularExpression.escapedPattern(for: commentToken(lang))
        apply((commentToken(lang) == "//" ? "(?<!:)" : "") + ct + ".*", UIColor(Theme.fgMuted))
        return ns
    }
}

extension NSAttributedString.Key { static let diffLineBG = NSAttributedString.Key("pom.diffLineBG") }

final class DiffLayoutManager: NSLayoutManager {
    override func drawBackground(forGlyphRange glyphsToShow: NSRange, at origin: CGPoint) {
        if let ts = textStorage, let tc = textContainers.first {
            let fillW = usedRect(for: tc).width + 4000
            let charRange = characterRange(forGlyphRange: glyphsToShow, actualGlyphRange: nil)
            ts.enumerateAttribute(.diffLineBG, in: charRange) { val, range, _ in
                guard let color = val as? UIColor else { return }
                let gr = glyphRange(forCharacterRange: range, actualCharacterRange: nil)
                enumerateLineFragments(forGlyphRange: gr) { rect, _, _, _, _ in
                    let r = CGRect(x: 0, y: rect.origin.y + origin.y, width: fillW, height: rect.height)
                    color.setFill()
                    UIBezierPath(rect: r).fill()
                }
            }
        }
        super.drawBackground(forGlyphRange: glyphsToShow, at: origin)
    }
}

private struct DiffTextView: UIViewRepresentable {
    let file: DiffFile

    func makeUIView(context: Context) -> UITextView {
        let storage = NSTextStorage()
        let lm = DiffLayoutManager()
        storage.addLayoutManager(lm)
        let container = NSTextContainer(size: CGSize(width: 1e7, height: 1e7))
        container.widthTracksTextView = false
        container.lineFragmentPadding = 0
        lm.addTextContainer(container)
        let tv = UITextView(frame: .zero, textContainer: container)
        tv.isEditable = false
        tv.isScrollEnabled = false
        tv.backgroundColor = .clear
        tv.textContainerInset = UIEdgeInsets(top: 4, left: 10, bottom: 4, right: 12)
        return tv
    }

    func updateUIView(_ tv: UITextView, context: Context) {
        tv.textStorage.setAttributedString(Self.render(file))
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: UITextView, context: Context) -> CGSize? {
        uiView.sizeThatFits(CGSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude))
    }

    private static func render(_ f: DiffFile) -> NSAttributedString {
        let lang = DiffSyntax.lang(for: f.path)
        let mono = UIFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        let gutterColor = UIColor(Theme.dim)
        let out = NSMutableAttributedString()
        func pad(_ n: Int?) -> String { let s = n.map(String.init) ?? ""; return String(repeating: " ", count: max(0, 4 - s.count)) + s }
        for ln in f.lines {
            let start = out.length
            let sign: String = ln.kind == .add ? "+" : (ln.kind == .del ? "-" : " ")
            out.append(NSAttributedString(string: "\(pad(ln.oldN)) \(pad(ln.newN)) \(sign) ",
                                          attributes: [.font: mono, .foregroundColor: gutterColor]))
            if ln.kind == .hunk {
                out.append(NSAttributedString(string: ln.text, attributes: [.font: mono, .foregroundColor: UIColor(Theme.accent)]))
            } else {
                out.append(DiffSyntax.nsHighlight(ln.text, lang: lang, font: mono, base: UIColor(Theme.fgSoft)))
            }
            out.append(NSAttributedString(string: "\n", attributes: [.font: mono]))
            if let bg = lineBG(ln.kind) {
                out.addAttribute(.diffLineBG, value: bg, range: NSRange(location: start, length: out.length - start))
            }
        }
        return out
    }

    private static func lineBG(_ k: DiffLine.Kind) -> UIColor? {
        switch k {
        case .add: return UIColor(Theme.ok.opacity(0.12))
        case .del: return UIColor(Theme.danger.opacity(0.12))
        case .hunk: return UIColor(Theme.accent.opacity(0.08))
        default: return nil
        }
    }
}

private struct PRFilesTab: View {
    let client: RemoteClient
    let branch: String
    let isMain: Bool
    let repo: String
    @State private var files: [DiffFile]?
    @State private var open: Set<String> = []

    var body: some View {
        ScrollView {
            if let files {
                if files.isEmpty {
                    empty("doc.on.doc", "No file changes")
                } else {
                    LazyVStack(spacing: 8) {
                        ForEach(files) { f in fileCard(f) }
                    }
                    .padding(12)
                }
            } else {
                ProgressView().tint(Theme.accent).padding(40)
            }
        }
        .background(Theme.bg)
        .task { await load(force: false) }
        .refreshable { await load(force: true) }
    }

    private func load(force: Bool) async {
        if !force && files != nil { return }
        if force { await client.prRefresh() }
        if let d = try? await client.prDiff(branch: branch, repo: repo, isMain: isMain) {
            files = PomJSON.decode([DiffFile].self, from: d) ?? []
        } else { files = [] }
    }

    private func fileCard(_ f: DiffFile) -> some View {
        let isOpen = open.contains(f.path)
        return VStack(alignment: .leading, spacing: 0) {
            Button {
                if isOpen { open.remove(f.path) } else { open.insert(f.path) }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: isOpen ? "chevron.down" : "chevron.right").font(.system(size: 9, weight: .semibold)).foregroundStyle(Theme.fgMuted)
                    statusTag(f.status)
                    Text(f.path).font(Theme.mono(11)).foregroundStyle(Theme.fg).lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 6)
                    Text("+\(f.adds)").font(Theme.mono(10)).foregroundStyle(Theme.ok)
                    Text("-\(f.dels)").font(Theme.mono(10)).foregroundStyle(Theme.danger)
                }
                .padding(10)
                .contentShape(Rectangle())
            }.buttonStyle(.plain)
            if isOpen {
                Divider().overlay(Theme.borderSoft)
                if f.binary {
                    Text("Binary file").font(Theme.mono(10)).foregroundStyle(Theme.fgMuted).padding(10)
                } else {
                    ScrollView(.horizontal, showsIndicators: true) {
                        DiffTextView(file: f)
                    }
                    .padding(.vertical, 4)
                }
            }
        }
        .background(Theme.bgSoft)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderSoft, lineWidth: 1))
    }

    private func statusTag(_ s: String) -> some View {
        let (t, c): (String, Color) = {
            switch s { case "A": return ("A", Theme.ok); case "D": return ("D", Theme.danger)
            case "R", "C": return ("R", Theme.warn); default: return ("M", Theme.accent) }
        }()
        return Text(t).font(Theme.mono(9, .semibold)).foregroundStyle(c)
            .frame(width: 16, height: 16).background(c.opacity(0.14), in: RoundedRectangle(cornerRadius: 4))
    }
}

private struct PRReviewComment: Decodable, Identifiable {
    var user: String?, avatarUrl: String?, body: String?, path: String?, line: Int?
    var hunkLines: [DiffLine] = []
    var id = UUID().uuidString
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        user = try c.decodeIfPresent(String.self, forKey: .user)
        avatarUrl = try c.decodeIfPresent(String.self, forKey: .avatarUrl)
        body = try c.decodeIfPresent(String.self, forKey: .body)
        path = try c.decodeIfPresent(String.self, forKey: .path)
        line = try c.decodeIfPresent(Int.self, forKey: .line)
        hunkLines = try c.decodeIfPresent([DiffLine].self, forKey: .hunkLines) ?? []
    }
    enum K: String, CodingKey { case user, avatarUrl, body, path, line, hunkLines }
}

private struct TimelinePayload: Decodable { var items: [PRTimelineItem] = [] }

private struct PRTimelineItem: Decodable, Identifiable {
    var id = ""
    var kind = ""
    var author: String?, avatar: String?
    var body = "", at = "", state = ""
    var inline: [PRReviewComment] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decodeIfPresent(String.self, forKey: .id) ?? UUID().uuidString
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? ""
        author = try c.decodeIfPresent(String.self, forKey: .author)
        avatar = try c.decodeIfPresent(String.self, forKey: .avatar)
        body = try c.decodeIfPresent(String.self, forKey: .body) ?? ""
        at = try c.decodeIfPresent(String.self, forKey: .at) ?? ""
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? ""
        inline = try c.decodeIfPresent([PRReviewComment].self, forKey: .inline) ?? []
    }
    enum K: String, CodingKey { case id, kind, author, avatar, body, at, state, inline }
}

private struct PRConversationTab: View {
    let client: RemoteClient
    let branch: String
    let isMain: Bool
    let repo: String
    @State private var items: [PRTimelineItem]?

    var body: some View {
        ScrollView {
            if let items {
                if items.isEmpty {
                    empty("bubble.left.and.bubble.right", "No conversation")
                } else {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(items.enumerated()), id: \.element.id) { i, it in
                            timelineRow(it, isLast: i == items.count - 1)
                        }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 12)
                }
            } else {
                ProgressView().tint(Theme.accent).padding(40)
            }
        }
        .background(Theme.bg)
        .task { await load(force: false) }
        .refreshable { await load(force: true) }
    }

    private func load(force: Bool) async {
        if !force && items != nil { return }
        if force { await client.prRefresh() }
        if let d = try? await client.prTimeline(branch: branch, repo: repo, isMain: isMain) {
            items = PomJSON.decode(TimelinePayload.self, from: d)?.items ?? []
        } else { items = [] }
    }

    private let avatarSize: CGFloat = 30

    @ViewBuilder private func timelineRow(_ it: PRTimelineItem, isLast: Bool) -> some View {
        HStack(alignment: .top, spacing: 10) {
            ZStack(alignment: .top) {
                VStack(spacing: 0) {
                    Color.clear.frame(height: avatarSize)
                    if !isLast { Rectangle().fill(Theme.borderSoft).frame(width: 2).frame(maxHeight: .infinity) }
                }
                avatarBadge(it)
            }
            .frame(width: avatarSize).frame(maxHeight: .infinity, alignment: .top)
            VStack(alignment: .leading, spacing: 8) {
                switch it.kind {
                case "review":
                    HStack(spacing: 6) {
                        Text(it.author ?? "someone").font(Theme.ui(12, .semibold)).foregroundStyle(Theme.fg)
                        Text(reviewVerb(it.state)).font(Theme.ui(12)).foregroundStyle(Theme.fgMuted)
                        Spacer()
                    }.frame(height: avatarSize)
                    if !it.body.isEmpty { commentCard(it.author, it.avatar, "left a comment", it.body) }
                    ForEach(it.inline) { inlineCard($0) }
                case "description":
                    commentCard(it.author, it.avatar, "opened this pull request", it.body)
                default:
                    commentCard(it.author, it.avatar, "commented", it.body)
                }
            }
            .padding(.bottom, 16)
        }
    }

    private func avatarBadge(_ it: PRTimelineItem) -> some View {
        ZStack(alignment: .bottomTrailing) {
            AvatarView(url: it.avatar ?? "", name: it.author ?? "", size: avatarSize)
            if it.kind == "review" {
                let col = reviewCol(it.state)
                Image(systemName: reviewIcon(it.state)).font(.system(size: 8, weight: .bold)).foregroundStyle(col)
                    .padding(2).background(Theme.bg, in: Circle())
                    .overlay(Circle().strokeBorder(Theme.bg, lineWidth: 1))
                    .offset(x: 2, y: 2)
            }
        }
    }

    private func commentCard(_ author: String?, _ avatar: String?, _ head: String, _ body: String) -> some View {
        Card(cornerRadius: 8) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    AvatarView(url: avatar ?? "", name: author ?? "", size: 18)
                    Text(author ?? "unknown").font(Theme.ui(11.5, .semibold)).foregroundStyle(Theme.fg)
                    Text(head).font(Theme.ui(11.5)).foregroundStyle(Theme.fgMuted)
                    Spacer()
                }
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(Theme.panel3)
                if !body.isEmpty {
                    Rectangle().fill(Theme.borderSoft).frame(height: 1)
                    MarkdownBlock(text: body).textSelection(.enabled).padding(12)
                }
            }
        }
    }

    private func inlineCard(_ rc: PRReviewComment) -> some View {
        Card(cornerRadius: 8) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Image(systemName: "doc.text").font(.system(size: 10)).foregroundStyle(Theme.fgMuted)
                    Text(rc.path.map { "\($0)\(rc.line.map { ":\($0)" } ?? "")" } ?? "")
                        .font(Theme.mono(10)).foregroundStyle(Theme.fg).lineLimit(1).truncationMode(.middle)
                    Spacer()
                }
                .padding(.horizontal, 10).padding(.vertical, 7)
                .background(Theme.panel3)
                if !rc.hunkLines.isEmpty {
                    Rectangle().fill(Theme.borderSoft).frame(height: 1)
                    hunkSnippet(rc.hunkLines)
                }
                Rectangle().fill(Theme.borderSoft).frame(height: 1)
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 6) {
                        AvatarView(url: rc.avatarUrl ?? "", name: rc.user ?? "", size: 18)
                        Text(rc.user ?? "unknown").font(Theme.ui(11, .semibold)).foregroundStyle(Theme.fg)
                    }
                    MarkdownBlock(text: rc.body ?? "").textSelection(.enabled)
                }
                .padding(10)
            }
        }
    }

    private func hunkSnippet(_ lines: [DiffLine]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(lines.suffix(6)) { l in
                let tint: Color? = l.kind == .add ? Theme.ok : (l.kind == .del ? Theme.danger : nil)
                let num = l.kind == .del ? l.oldN : l.newN
                HStack(spacing: 0) {
                    Text(num.map(String.init) ?? "").font(Theme.mono(10)).foregroundStyle(Theme.dim)
                        .frame(width: 34, alignment: .trailing)
                    Text(l.kind == .add ? "+" : (l.kind == .del ? "-" : " ")).font(Theme.mono(10.5, .bold))
                        .foregroundStyle(tint ?? Theme.dim).frame(width: 14)
                    Text(l.text.isEmpty ? " " : l.text).font(Theme.mono(10.5)).foregroundStyle(Theme.fgSoft)
                        .lineLimit(1).truncationMode(.tail)
                    Spacer(minLength: 0)
                }
                .padding(.trailing, 8).padding(.vertical, 1.5)
                .background(tint?.opacity(0.10) ?? .clear)
            }
        }
        .padding(.vertical, 4)
        .background(Theme.bg)
    }

    private func reviewVerb(_ s: String) -> String {
        switch s { case "APPROVED": return "approved these changes"; case "CHANGES_REQUESTED": return "requested changes"; case "DISMISSED": return "review dismissed"; default: return "reviewed" }
    }
    private func reviewIcon(_ s: String) -> String {
        switch s { case "APPROVED": return "checkmark.circle.fill"; case "CHANGES_REQUESTED": return "xmark.circle.fill"; default: return "eye" }
    }
    private func reviewCol(_ s: String) -> Color {
        switch s { case "APPROVED": return Theme.ok; case "CHANGES_REQUESTED": return Theme.danger; default: return Theme.fgMuted }
    }
}

private func empty(_ icon: String, _ text: String) -> some View {
    VStack(spacing: 8) {
        Image(systemName: icon).font(.system(size: 26)).foregroundStyle(Theme.fgMuted)
        Text(text).font(Theme.ui(13)).foregroundStyle(Theme.fgMuted)
    }
    .frame(maxWidth: .infinity).padding(.top, 60)
}
