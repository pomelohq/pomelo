import SwiftUI

struct JiraComment: Decodable, Equatable, Identifiable {
    var commentId = ""
    var author = ""
    var avatar = ""
    var created = ""
    var body = ""
    var id: String { commentId.isEmpty ? author + created : commentId }
    var when: String {
        let s = created.replacingOccurrences(of: "T", with: " ")
        return String(s.prefix(16))
    }
    // Jira style: "August 14, 2026 at 11:35 AM"
    var whenLong: String { JiraDates.long(created) ?? when }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        commentId = try c.decodeIfPresent(String.self, forKey: .commentId) ?? ""
        author = try c.decodeIfPresent(String.self, forKey: .author) ?? ""
        avatar = try c.decodeIfPresent(String.self, forKey: .avatar) ?? ""
        created = try c.decodeIfPresent(String.self, forKey: .created) ?? ""
        body = try c.decodeIfPresent(String.self, forKey: .body) ?? ""
    }
    enum K: String, CodingKey { case commentId = "id", author, avatar, created, body }
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

struct JiraWebLink: Decodable, Equatable, Identifiable {
    var title = ""
    var url = ""
    var icon = ""
    var id: String { url }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        icon = try c.decodeIfPresent(String.self, forKey: .icon) ?? ""
    }
    enum K: String, CodingKey { case title, url, icon }
}

struct JiraDetail: Decodable, Equatable {
    var configured = false
    var key = ""
    var summary = ""
    var status = ""
    var url = ""
    var description = ""
    var comments: [JiraComment] = []
    var webLinks: [JiraWebLink] = []
    var error: String?

    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        configured = try c.decodeIfPresent(Bool.self, forKey: .configured) ?? false
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        description = try c.decodeIfPresent(String.self, forKey: .description) ?? ""
        comments = try c.decodeIfPresent([JiraComment].self, forKey: .comments) ?? []
        webLinks = try c.decodeIfPresent([JiraWebLink].self, forKey: .webLinks) ?? []
        error = try c.decodeIfPresent(String.self, forKey: .error)
    }

    enum CodingKeys: String, CodingKey {
        case configured, key, summary, status, url, description, comments, error
        case webLinks = "web_links"
    }
}

struct JiraPane: View {
    @Environment(AppState.self) var state
    @EnvironmentObject var theme: ThemeManager
    let workspace: Workspace

    @StateObject private var vm = JiraPaneViewModel()
    @State private var showComments = true
    @AppStorage("jira.commentsWidth") private var commentsWidth = 400.0
    @State private var collapsedComments: Set<String> = []

    private var key: String? { jiraKey(workspace.branch) }
    private var catColor: Color { state.jiraFor(workspace.branch)?.color ?? Theme.accent }

    var body: some View {
        Group {
            if key == nil {
                empty("No Jira ticket", "This branch name has no ABC-123 key.")
            } else if let d = vm.detail {
                if !d.configured {
                    empty("Jira not configured", "Add a jira: block + token to pom.yml.")
                } else if d.error != nil || d.key.isEmpty {
                    empty("Ticket not found", key ?? "")
                } else {
                    content(d)
                }
            } else if vm.loading {
                VStack(spacing: 8) { Spinner(size: 14); Text("loading ticket…").font(.system(size: 12)).foregroundStyle(Theme.fgMuted) }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                empty("Jira", "—")
            }
        }
        .background(Theme.bg)
        .task(id: workspace.id) { await vm.load(key: key) }
    }

    private func content(_ d: JiraDetail) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text(d.key).font(Theme.mono(12, .semibold)).foregroundStyle(catColor)
                StatusPill(text: d.status, color: catColor)
                Spacer()
                if !d.comments.isEmpty {
                    Button { withAnimation(.easeInOut(duration: 0.15)) { showComments.toggle() } } label: {
                        HStack(spacing: 4) {
                            Image(systemName: showComments ? "sidebar.right" : "bubble.left.and.bubble.right")
                            Text("\(d.comments.count)").font(.system(size: 12, weight: .medium))
                        }.foregroundStyle(showComments ? catColor : Theme.fgMuted)
                    }.buttonStyle(.plain).help(showComments ? "Hide comments (⌘⇧C)" : "Show comments (⌘⇧C)")
                        .keyboardShortcut("c", modifiers: [.command, .shift])
                }
                Button { Task { await vm.reload(key: key) } } label: {
                    Image(systemName: "arrow.clockwise").font(.system(size: 12))
                        .foregroundStyle(Theme.fgMuted)
                        .rotationEffect(.degrees(vm.reloading ? 360 : 0))
                        .animation(vm.reloading ? .linear(duration: 0.7).repeatForever(autoreverses: false) : .default, value: vm.reloading)
                }.buttonStyle(.plain).help("Reload from Jira").disabled(vm.reloading)
                Button { if let u = URL(string: d.url) { NSWorkspace.shared.open(u) } } label: {
                    HStack(spacing: 4) { Image(systemName: "arrow.up.forward.square"); Text("Open").font(.system(size: 12)) }
                        .foregroundStyle(Theme.fgMuted)
                }.buttonStyle(.plain).help("Open in Jira")
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            Divider().overlay(Theme.borderSoft)
            HStack(spacing: 0) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        Text(d.summary).font(.system(size: 16, weight: .semibold)).foregroundStyle(Theme.fg)
                            .fixedSize(horizontal: false, vertical: true)
                        if d.description.isEmpty {
                            Text("No description.").font(.system(size: 12)).foregroundStyle(Theme.dim)
                        } else {
                            MarkdownText(d.description)
                        }
                        if !d.webLinks.isEmpty {
                            webLinksSection(d.webLinks)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
                    .readingColumn()
                }
                if showComments && !d.comments.isEmpty {
                    SplitHandle(axis: .horizontal, value: $commentsWidth, min: 300, max: 900, invert: true)
                    commentsPane(d.comments).frame(width: commentsWidth)
                }
            }
        }
    }

    private func webLinksSection(_ links: [JiraWebLink]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            SectionLabel(text: "WEB LINKS")
            Card(cornerRadius: 6, background: Theme.bgSoft) {
                VStack(spacing: 0) {
                    ForEach(links) { link in
                        Button {
                            if let u = URL(string: link.url) { NSWorkspace.shared.open(u) }
                        } label: {
                            HStack(spacing: 8) {
                                webLinkIcon(link.icon)
                                Text(link.title.isEmpty ? link.url : link.title)
                                    .font(.system(size: 12.5)).foregroundStyle(Theme.fg).lineLimit(1)
                                Spacer()
                                Image(systemName: "arrow.up.forward.square").font(.system(size: 10.5)).foregroundStyle(Theme.dim)
                            }
                            .padding(.horizontal, 10).padding(.vertical, 8)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        if link.id != links.last?.id {
                            Divider().overlay(Theme.borderSoft)
                        }
                    }
                }
            }
        }
    }

    private func webLinkIcon(_ icon: String) -> some View {
        Group {
            if let u = URL(string: icon), !icon.isEmpty {
                AsyncImage(url: u) { phase in
                    if let img = phase.image {
                        img.resizable()
                    } else {
                        Image(systemName: "link").foregroundStyle(Theme.dim)
                    }
                }
            } else {
                Image(systemName: "link").foregroundStyle(Theme.dim)
            }
        }
        .frame(width: 14, height: 14)
    }

    private func commentsPane(_ comments: [JiraComment]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                SectionLabel(text: "COMMENTS")
                Text("\(comments.count)").font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
                Spacer()
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            Divider().overlay(Theme.borderSoft)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(visibleRows(comments), id: \.c.id) { row in
                        commentRow(row)
                        if row.parent == nil, !(row.replies > 0 && !collapsedComments.contains(row.c.id)) {
                            Divider().overlay(Theme.borderSoft).padding(.horizontal, 14)
                        } else if row.last {
                            Divider().overlay(Theme.borderSoft).padding(.horizontal, 14)
                        }
                    }
                }
            }
        }
        .background(Theme.bgSoft)
    }

    private struct Row { let c: JiraComment; let parent: String?; var replies = 0; var last = false }

    // Jira has no reply threads in the API; a comment that opens with **@Name**
    // is treated as a reply to that person's latest earlier comment.
    private func threaded(_ all: [JiraComment]) -> [Row] {
        func mentioned(_ body: String) -> String? {
            let t = body.trimmingCharacters(in: .whitespacesAndNewlines)
            guard t.hasPrefix("**@"), let end = t.range(of: "**", range: t.index(t.startIndex, offsetBy: 3)..<t.endIndex)
            else { return nil }
            return String(t[t.index(t.startIndex, offsetBy: 3)..<end.lowerBound])
        }
        let chrono = all.sorted { $0.created < $1.created }
        var children: [String: [JiraComment]] = [:]
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

    private func visibleRows(_ all: [JiraComment]) -> [Row] {
        threaded(all).filter { $0.parent == nil || !collapsedComments.contains($0.parent!) }
    }

    private let avatarSize: CGFloat = 32
    private let replyAvatar: CGFloat = 24
    private let railX: CGFloat = 14 + 16   // centre of the top-level avatar

    private func commentRow(_ row: Row) -> some View {
        let c = row.c
        let isReply = row.parent != nil
        let collapsed = collapsedComments.contains(c.id)
        let showsRail = (!isReply && row.replies > 0 && !collapsed) || isReply
        return HStack(alignment: .top, spacing: 10) {
            if isReply { Color.clear.frame(width: railX + 8) }
            Avatar(url: c.avatar.isEmpty ? nil : c.avatar, name: c.author, size: isReply ? replyAvatar : avatarSize, viaCore: true)
                .padding(.top, isReply ? 2 : 0)
            VStack(alignment: .leading, spacing: 4) {
                Text(c.author.isEmpty ? "—" : c.author).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.fg).lineLimit(1)
                Text(c.whenLong).font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                if c.body.isEmpty {
                    Text("—").font(.system(size: 12)).foregroundStyle(Theme.dim)
                } else {
                    MarkdownText(c.body).padding(.top, 4)
                }
                if !isReply, row.replies > 0 {
                    Button {
                        withAnimation(.easeInOut(duration: 0.12)) {
                            if collapsed { collapsedComments.remove(c.id) } else { collapsedComments.insert(c.id) }
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Image(systemName: collapsed ? "chevron.right" : "chevron.down").font(.system(size: 9, weight: .semibold))
                            Text("\(row.replies) \(row.replies == 1 ? "reply" : "replies")").font(.system(size: 11.5, weight: .medium))
                        }.foregroundStyle(Theme.accent)
                    }.buttonStyle(.plain).padding(.top, 6)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.leading, isReply ? 0 : 14).padding(.trailing, 14)
        .padding(.top, isReply ? 6 : 16).padding(.bottom, isReply && !row.last ? 6 : 16)
        .background(alignment: .topLeading) {
            if showsRail { threadRail(isReply: isReply, last: row.last) }
        }
    }

    // The connector Jira draws: straight down from the parent's avatar, curving
    // into each reply's avatar; ends after the last reply.
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

    private func empty(_ title: String, _ sub: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: "ticket").font(.system(size: 30)).foregroundStyle(Theme.dim)
            Text(title).font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
            Text(sub).font(.system(size: 11)).foregroundStyle(Theme.dim)
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }

}
