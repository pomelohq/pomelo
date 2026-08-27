import SwiftUI

struct JiraComment: Decodable, Equatable, Identifiable {
    var author = ""
    var created = ""
    var body = ""
    var id: String { author + created }
    var when: String {
        let s = created.replacingOccurrences(of: "T", with: " ")
        return String(s.prefix(16))
    }
}

struct JiraWebLink: Decodable, Equatable, Identifiable {
    var title = ""
    var url = ""
    var icon = ""
    var id: String { url }
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

    enum CodingKeys: String, CodingKey {
        case configured, key, summary, status, url, description, comments, error
        case webLinks = "web_links"
    }
}

struct JiraPane: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    let workspace: Workspace

    @StateObject private var vm = JiraPaneViewModel()
    @State private var showComments = true
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
                VStack(spacing: 8) { ProgressView().controlSize(.small); Text("loading ticket…").font(.system(size: 12)).foregroundStyle(Theme.fgMuted) }
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
                Text(d.status).font(.system(size: 10.5, weight: .medium)).foregroundStyle(catColor)
                    .padding(.horizontal, 7).padding(.vertical, 2).background(catColor.opacity(0.15), in: Capsule())
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
                }
                if showComments && !d.comments.isEmpty {
                    Divider().overlay(Theme.borderSoft)
                    commentsPane(d.comments).frame(width: 360)
                }
            }
        }
    }

    private func webLinksSection(_ links: [JiraWebLink]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("WEB LINKS").font(.system(size: 10.5, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
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
            .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Theme.borderSoft, lineWidth: 1))
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
                Text("COMMENTS").font(.system(size: 10.5, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
                Text("\(comments.count)").font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
                Spacer()
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            Divider().overlay(Theme.borderSoft)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(visibleRows(comments), id: \.c.id) { row in
                        commentRow(row.c, parent: row.parent)
                    }
                }
            }
        }
        .background(Theme.bgSoft)
    }

    private struct Row { let c: JiraComment; let parent: String? }

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
            out.append(Row(c: top, parent: nil))
            for r in (children[top.id] ?? []) { out.append(Row(c: r, parent: top.id)) }
        }
        return out
    }

    private func visibleRows(_ all: [JiraComment]) -> [Row] {
        threaded(all).filter { $0.parent == nil || !collapsedComments.contains($0.parent!) }
    }

    private func commentRow(_ c: JiraComment, parent: String?) -> some View {
        let collapsed = collapsedComments.contains(c.id)
        let isReply = parent != nil
        return VStack(alignment: .leading, spacing: 5) {
            Button {
                if collapsed { collapsedComments.remove(c.id) } else { collapsedComments.insert(c.id) }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: collapsed ? "chevron.right" : "chevron.down")
                        .font(.system(size: 8)).foregroundStyle(Theme.dim).frame(width: 10)
                    Text(c.author.isEmpty ? "—" : c.author).font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.fg).lineLimit(1)
                    Spacer(minLength: 6)
                    Text(c.when).font(Theme.mono(10)).foregroundStyle(Theme.dim)
                }.contentShape(Rectangle())
            }.buttonStyle(.plain)
            if !collapsed {
                if c.body.isEmpty {
                    Text("—").font(.system(size: 11.5)).foregroundStyle(Theme.dim)
                } else {
                    MarkdownText(c.body)
                }
            }
        }
        .padding(.vertical, 9).padding(.trailing, 14)
        .padding(.leading, isReply ? 26 : 14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .leading) {
            if isReply { Rectangle().fill(Theme.borderSoft).frame(width: 1).padding(.leading, 14).padding(.vertical, 2) }
        }
        .overlay(alignment: .bottom) {
            if !isReply { Rectangle().fill(Theme.borderSoft).frame(height: 1) }
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
