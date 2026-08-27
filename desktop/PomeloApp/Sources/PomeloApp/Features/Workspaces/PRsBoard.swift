import SwiftUI

struct WorkspacePR: Decodable, Identifiable, Equatable {
    let repo: String
    let alias: String
    var behind: Int = 0
    var ahead: Int = 0
    let pr: PRInfo?
    var id: String { repo }
}

struct LocalChangeRepo: Decodable, Identifiable, Equatable {
    let repo: String
    let alias: String
    var files: Int = 0
    var insertions: Int = 0
    var deletions: Int = 0
    var behind: Int = 0
    var id: String { repo }
}

struct PRInfo: Decodable, Equatable {
    let number: Int
    let title: String
    let state: String
    let url: String
    var isDraft: Bool = false
    var mergeable: String?
    var reviewDecision: String?
    var body: String?
    var headRefName: String?
    var baseRefName: String?
    var additions: Int?
    var deletions: Int?
    var changedFiles: Int?
    var author: Author?
    var reviews: [Review]?
    var reviewRequests: [ReviewRequest]?
    var comments: [Comment]?
    var labels: [Label]?
    var statusCheckRollup: [Check]?

    struct Author: Decodable, Equatable { var login: String? }
    struct ReviewRequest: Decodable, Equatable { var login: String?; var name: String?; var slug: String? }

    struct Reviewer: Identifiable, Equatable { let name: String; let state: String; var id: String { name } }
    var reviewers: [Reviewer] {
        var latest: [String: String] = [:]
        for r in reviews ?? [] {
            guard let who = r.author?.login, !who.isEmpty else { continue }
            latest[who] = (r.state ?? "").uppercased()
        }
        var out: [Reviewer] = []
        for (who, st) in latest {
            let s = st == "APPROVED" ? "approved" : st == "CHANGES_REQUESTED" ? "changes" : "commented"
            out.append(Reviewer(name: who, state: s))
        }
        let done = Set(latest.keys)
        for rr in reviewRequests ?? [] {
            let who = rr.login ?? rr.name ?? ""
            if !who.isEmpty && !done.contains(who) { out.append(Reviewer(name: who, state: "pending")) }
        }
        return out.sorted { $0.name < $1.name }
    }
    struct Label: Decodable, Equatable, Identifiable { var name: String?; var color: String?; var id: String { name ?? "" } }

    struct Check: Decodable, Equatable, Identifiable {
        var name: String?
        var context: String?
        var conclusion: String?
        var state: String?
        var status: String?
        var detailsUrl: String?
        var targetUrl: String?
        var workflowName: String?
        var id: String { (name ?? context ?? "check") + (conclusion ?? state ?? status ?? "") }
        var label: String { name ?? context ?? "check" }
        var link: String? { detailsUrl ?? targetUrl }
        var result: ChecksStatus {
            let v = (conclusion ?? state ?? "").uppercased()
            let s = (status ?? "").uppercased()
            if v == "FAILURE" || v == "ERROR" || v == "TIMED_OUT" || v == "CANCELLED" { return .fail }
            if v == "PENDING" || s == "IN_PROGRESS" || s == "QUEUED" || s == "PENDING" { return .pending }
            if v == "SUCCESS" { return .pass }
            return .none
        }
    }
    struct Review: Decodable, Equatable, Identifiable {
        var author: Author?
        var state: String?
        var body: String?
        var submittedAt: String?
        var id: String { (author?.login ?? "?") + (state ?? "") + (submittedAt ?? "") + String((body ?? "").prefix(12)) }
    }
    struct Comment: Decodable, Equatable, Identifiable {
        var author: Author?
        var body: String?
        var createdAt: String?
        var id: String { (author?.login ?? "?") + String((body ?? "").prefix(24)) }
    }

    enum ChecksStatus { case pass, fail, pending, none }
    var checks: ChecksStatus {
        guard let c = statusCheckRollup, !c.isEmpty else { return .none }
        var pending = false, pass = false
        for x in c {
            switch x.result {
            case .fail: return .fail
            case .pending: pending = true
            case .pass: pass = true
            case .none: break
            }
        }
        if pending { return .pending }
        return pass ? .pass : .none
    }

    enum ReviewDecision { case approved, changes, review, none }
    var review: ReviewDecision {
        switch (reviewDecision ?? "").uppercased() {
        case "APPROVED": return .approved
        case "CHANGES_REQUESTED": return .changes
        case "REVIEW_REQUIRED": return .review
        default: break
        }
        guard let r = reviews, !r.isEmpty else { return .none }
        var approved = false
        for x in r {
            switch (x.state ?? "").uppercased() {
            case "CHANGES_REQUESTED": return .changes
            case "APPROVED": approved = true
            default: break
            }
        }
        return approved ? .approved : .review
    }

    var conflict: Bool { (mergeable ?? "").uppercased() == "CONFLICTING" }
}

typealias PRCheck = PRInfo.Check
typealias PRReview = PRInfo.Review

struct PRCommit: Decodable, Identifiable, Equatable {
    let hash: String; let subject: String
    var author: String = ""; var date: String = ""
    var id: String { hash }
}

struct PRReviewComment: Decodable, Identifiable, Equatable {
    var user: String?; var body: String?; var path: String?; var line: Int?
    var diffHunk: String?; var createdAt: String?
    var id: String { (user ?? "") + (path ?? "") + String(line ?? 0) + String((body ?? "").prefix(12)) }
}

struct PRsBoard: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    let workspace: Workspace

    enum Selection: Equatable { case pr(String), local(String) }

    @State private var prs: [WorkspacePR] = []
    @State private var localChanges: [LocalChangeRepo] = []
    @State private var selection: Selection?
    @State private var loading = true
    @State private var pollTask: Task<Void, Never>?
    @AppStorage("prs.masterWidth") private var masterWidth = 320.0
    @AppStorage("prs.sectionExpanded") private var sectionExpanded = true
    @AppStorage("prs.localSectionExpanded") private var localSectionExpanded = true

    var body: some View {
        HStack(spacing: 0) {
            master.frame(width: masterWidth)
            SplitHandle(axis: .horizontal, value: $masterWidth, min: 220, max: 640)
            Group {
                switch selection {
                case .pr(let repo):
                    if let it = prs.first(where: { $0.repo == repo }) {
                        PRDetail(item: it, branch: workspace.branch, isMain: workspace.isMain).id("pr:\(it.repo)")
                    } else { emptyDetail }
                case .local(let repo):
                    if let it = localChanges.first(where: { $0.repo == repo }) {
                        LocalChangesDetail(item: it, branch: workspace.branch, isMain: workspace.isMain).id("local:\(it.repo)")
                    } else { emptyDetail }
                case nil:
                    emptyDetail
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(Theme.bg)
        .task(id: workspace.id) { await load(); startPolling() }
        .onDisappear { pollTask?.cancel() }
    }

    private var master: some View {
        VStack(spacing: 0) {
            localChangesSection
            prSection
        }
        .background(Theme.bgSoft)
    }

    @ViewBuilder private var localChangesSection: some View {
        Button { withAnimation(.easeInOut(duration: 0.14)) { localSectionExpanded.toggle() } } label: {
            HStack(spacing: 8) {
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(Theme.muted)
                    .rotationEffect(.degrees(localSectionExpanded ? 90 : 0))
                Text("LOCAL CHANGES").font(.system(size: 10.5, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
                if !localChanges.isEmpty {
                    Text("\(localChanges.count)").font(Theme.mono(9.5)).foregroundStyle(Theme.fgMuted)
                        .padding(.horizontal, 5).padding(.vertical, 1).background(Theme.dim.opacity(0.15), in: Capsule())
                }
                Spacer()
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .contentShape(Rectangle())
        }.buttonStyle(.plain)
        .background(Theme.bgSoft)
        Divider().overlay(Theme.borderSoft)

        if localSectionExpanded {
            if localChanges.isEmpty {
                Text("No local changes").font(.system(size: 12)).foregroundStyle(Theme.dim)
                    .padding(.horizontal, 14).padding(.vertical, 10)
            } else {
                LazyVStack(spacing: 4) {
                    ForEach(localChanges) { item in
                        LocalChangeRow(item: item, active: selection == .local(item.repo))
                            .contentShape(Rectangle())
                            .onTapGesture { selection = .local(item.repo) }
                    }
                }
                .padding(8)
            }
        }
    }

    private var prSection: some View {
        VStack(spacing: 0) {
            Button { withAnimation(.easeInOut(duration: 0.14)) { sectionExpanded.toggle() } } label: {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(Theme.muted)
                        .rotationEffect(.degrees(sectionExpanded ? 90 : 0))
                    Text("PULL REQUESTS").font(.system(size: 10.5, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
                    if loading { ProgressView().controlSize(.mini) }
                    Spacer()
                    Button { Task { await load() } } label: {
                        Image(systemName: "arrow.clockwise").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                    }.buttonStyle(.plain).help("Refresh")
                }
                .padding(.horizontal, 14).padding(.vertical, 8)
                .contentShape(Rectangle())
            }.buttonStyle(.plain)
            .background(Theme.bgSoft)
            Divider().overlay(Theme.borderSoft)

            if sectionExpanded {
                if prs.isEmpty && !loading {
                    VStack(spacing: 8) {
                        Image(systemName: "arrow.triangle.pull").font(.system(size: 26)).foregroundStyle(Theme.dim)
                        Text("No pull requests").font(.system(size: 12.5)).foregroundStyle(Theme.fgMuted)
                    }.frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    ScrollView {
                        LazyVStack(spacing: 4) {
                            ForEach(prs) { item in
                                PRRow(item: item, active: selection == .pr(item.repo))
                                    .contentShape(Rectangle())
                                    .onTapGesture { selection = .pr(item.repo) }
                            }
                        }
                        .padding(8)
                    }
                }
            } else {
                Spacer(minLength: 0)
            }
        }
    }

    private var emptyDetail: some View {
        VStack(spacing: 10) {
            Image(systemName: "sidebar.right").font(.system(size: 30)).foregroundStyle(Theme.dim)
            Text("Select a pull request").font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func load() async {
        let branch = workspace.branch, isMain = workspace.isMain
        async let prsFetch: [WorkspacePR]? = Task.detached(priority: .userInitiated) {
            let d = PRStore.workspace(branch: branch, isMain: isMain)
            struct R: Decodable { let prs: [WorkspacePR]? }
            return (PomJSON.decode(R.self, from: d))?.prs
        }.value
        async let localFetch: [LocalChangeRepo]? = Task.detached(priority: .userInitiated) {
            let d = PRStore.localChanges(branch: branch, isMain: isMain)
            struct R: Decodable { let repos: [LocalChangeRepo]? }
            return (PomJSON.decode(R.self, from: d))?.repos
        }.value
        let (freshPRs, freshLocal) = await (prsFetch, localFetch)
        loading = false
        if let freshPRs, freshPRs != prs { prs = freshPRs }
        if let freshLocal, freshLocal != localChanges { localChanges = freshLocal }
        switch selection {
        case .pr(let repo) where !prs.contains(where: { $0.repo == repo }):
            selection = prs.first.map { .pr($0.repo) }
        case .local(let repo) where !localChanges.contains(where: { $0.repo == repo }):
            selection = prs.first.map { .pr($0.repo) }
        case nil:
            selection = prs.first.map { .pr($0.repo) }
        default:
            break
        }
    }

    private func startPolling() {
        pollTask?.cancel()
        pollTask = Task { [id = workspace.id] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 30_000_000_000)
                if state.appActive && state.selection == id { await load() }
            }
        }
    }
}

struct PRRow: View {
    @EnvironmentObject var theme: ThemeManager
    let item: WorkspacePR
    let active: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            statusDot.padding(.top, 3)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(item.alias).font(Theme.mono(11.5, .medium)).foregroundStyle(Theme.fg)
                    if let pr = item.pr { Text(verbatim: "#\(pr.number)").font(Theme.mono(10.5)).foregroundStyle(Theme.dim) }
                    Spacer(minLength: 0)
                }
                Text(item.pr?.title ?? "no open PR")
                    .font(.system(size: 12)).foregroundStyle(item.pr == nil ? Theme.dim : Theme.fgMuted)
                    .lineLimit(3).fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if let pr = item.pr { badges(pr) }
            }
        }
        .padding(.horizontal, 8).padding(.vertical, 7)
        .background(active ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 7))
    }

    @ViewBuilder private func badges(_ pr: PRInfo) -> some View {
        HStack(spacing: 5) {
            if pr.state == "MERGED" {
                mini("arrow.triangle.merge", "merged", Color(hex: 0xa371f7))   // terminal — CI/review no longer relevant
                Spacer(minLength: 0)
            } else if pr.state == "CLOSED" {
                mini("xmark", "closed", Theme.dim)
                Spacer(minLength: 0)
            } else {
                openBadges(pr)
            }
        }
    }

    @ViewBuilder private func openBadges(_ pr: PRInfo) -> some View {
        Group {
            if pr.isDraft { mini("pencil", "draft", Theme.dim) }
            switch pr.checks {
            case .pass:    mini("checkmark.circle.fill", "CI", Theme.ok)
            case .fail:    mini("xmark.circle.fill", "CI", Theme.danger)
            case .pending: mini("clock.fill", "CI", Theme.warn)
            case .none:    EmptyView()
            }
            switch pr.review {
            case .approved: mini("checkmark.seal.fill", "approved", Theme.ok)
            case .changes:  mini("exclamationmark.triangle.fill", "changes", Theme.danger)
            case .review:   mini("eye.fill", "review", Theme.fgMuted)
            case .none:     EmptyView()
            }
            if pr.conflict { mini("arrow.triangle.merge", "conflict", Theme.danger) }
            Spacer(minLength: 0)
        }
    }
    private func mini(_ icon: String, _ text: String, _ c: Color) -> some View {
        HStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 8.5))
            Text(text).font(.system(size: 9.5, weight: .medium))
        }
        .foregroundStyle(c)
        .padding(.horizontal, 5).padding(.vertical, 1)
        .background(c.opacity(0.13), in: Capsule())
    }

    private var statusDot: some View {
        let col: Color = {
            guard let pr = item.pr else { return Theme.dim }
            switch pr.state { case "MERGED": return Color(hex: 0xa371f7); case "CLOSED": return Theme.dim; default: break }
            if pr.isDraft { return Theme.dim }
            if pr.conflict { return Theme.danger }
            switch pr.checks { case .fail: return Theme.danger; case .pending: return Theme.warn; default: break }
            return Theme.ok
        }()
        return Circle().fill(col).frame(width: 8, height: 8)
    }
}

struct LocalChangeRow: View {
    @EnvironmentObject var theme: ThemeManager
    let item: LocalChangeRepo
    let active: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Circle().fill(item.files > 0 ? Theme.warn : Theme.accent)
                .frame(width: 8, height: 8).padding(.top, 3)
            VStack(alignment: .leading, spacing: 3) {
                Text(item.alias).font(Theme.mono(11.5, .medium)).foregroundStyle(Theme.fg)
                HStack(spacing: 6) {
                    if item.files > 0 {
                        Text("\(item.files)").font(Theme.mono(10.5)).foregroundStyle(Theme.fgMuted)
                        Text("+\(item.insertions)").font(Theme.mono(10)).foregroundStyle(Theme.ok)
                        Text("-\(item.deletions)").font(Theme.mono(10)).foregroundStyle(Theme.danger)
                    }
                    Spacer(minLength: 0)
                }
                if item.behind > 0 {
                    HStack(spacing: 4) {
                        Image(systemName: "arrow.down.circle").font(.system(size: 9))
                        Text("\(item.behind) behind — update from origin").font(.system(size: 10))
                    }
                    .foregroundStyle(Theme.warn)
                }
            }
        }
        .padding(.horizontal, 8).padding(.vertical, 7)
        .background(active ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 7))
    }
}

struct LocalChangesDetail: View {
    @EnvironmentObject var theme: ThemeManager
    let item: LocalChangeRepo
    let branch: String
    let isMain: Bool

    @State private var diffFiles: [DiffFile]?
    @State private var selFile: String?
    @State private var splitDiff = false
    @AppStorage("prs.filesTreeVisible") private var filesTreeVisible = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(Theme.borderSoft)
            DiffFilesView(files: diffFiles, selFile: $selFile, filesTreeVisible: $filesTreeVisible, splitDiff: $splitDiff,
                          loadingLabel: "loading diff…", emptyLabel: "No local changes")
        }
        .task(id: item.repo) { await loadDiff() }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text("local").font(.system(size: 10, weight: .semibold)).foregroundStyle(Theme.warn)
                .padding(.horizontal, 7).padding(.vertical, 2).background(Theme.warn.opacity(0.15), in: Capsule())
            Text(item.alias).font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            Spacer()
            Text("+\(item.insertions)").font(Theme.mono(11)).foregroundStyle(Theme.ok)
            Text("-\(item.deletions)").font(Theme.mono(11)).foregroundStyle(Theme.danger)
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
    }

    private func loadDiff() async {
        guard diffFiles == nil else { return }
        let repo = item.repo
        let files = await Task.detached(priority: .userInitiated) { () -> [DiffFile] in
            PomJSON.decode([DiffFile].self, from: PRStore.localDiff(branch: branch, repo: repo, isMain: isMain)) ?? []
        }.value
        diffFiles = files
        if selFile == nil { selFile = files.first?.path }
    }
}

struct PRDetail: View {
    @EnvironmentObject var theme: ThemeManager
    let item: WorkspacePR
    let branch: String
    let isMain: Bool

    enum Tab: String, CaseIterable { case overview = "Overview", files = "Files", commits = "Commits", checks = "Checks", conversation = "Conversation" }
    @State private var tab: Tab = .overview
    @State private var detail: PRInfo?
    @State private var diffFiles: [DiffFile]?
    @State private var selFile: String?
    @State private var splitDiff = false
    @AppStorage("prs.filesTreeVisible") private var filesTreeVisible = true
    @State private var commits: [PRCommit]?
    @State private var reviewComments: [PRReviewComment]?
    @State private var loadingDetail = true

    private var pr: PRInfo? { detail ?? item.pr }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(Theme.borderSoft)
            tabBar
            Divider().overlay(Theme.borderSoft)
            tabContent
        }
        .task(id: item.repo) { await loadDetail() }
    }

    @ViewBuilder private var header: some View {
        if let pr {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    statePill(pr)
                    Text(verbatim: "#\(pr.number)").font(Theme.mono(12)).foregroundStyle(Theme.dim)
                    Spacer()
                    Button { if let u = URL(string: pr.url) { NSWorkspace.shared.open(u) } } label: {
                        HStack(spacing: 4) { Image(systemName: "arrow.up.forward.square"); Text("Open").font(.system(size: 12)) }
                            .foregroundStyle(Theme.fgMuted)
                    }.buttonStyle(.plain).help("Open on GitHub")
                }
                Text(pr.title).font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg).lineLimit(2)
                HStack(spacing: 8) {
                    if let a = pr.author?.login { Label(a, systemImage: "person.crop.circle").labelStyle(.titleAndIcon).font(.system(size: 11)).foregroundStyle(Theme.fgMuted) }
                    if let h = pr.headRefName, let b = pr.baseRefName {
                        Text("\(h) → \(b)").font(Theme.mono(10.5)).foregroundStyle(Theme.dim).lineLimit(1)
                    }
                    Spacer()
                    if let a = pr.additions, let d = pr.deletions {
                        Text("+\(a)").font(Theme.mono(11)).foregroundStyle(Theme.ok)
                        Text("-\(d)").font(Theme.mono(11)).foregroundStyle(Theme.danger)
                    }
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
        } else {
            Text(loadingDetail ? "loading…" : "no open PR").font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
                .padding(16)
        }
    }

    private var tabBar: some View {
        HStack(spacing: 2) {
            ForEach(Tab.allCases, id: \.self) { t in
                Button { tab = t } label: {
                    Text(t.rawValue).font(.system(size: 12, weight: tab == t ? .semibold : .regular))
                        .foregroundStyle(tab == t ? Theme.accent : Theme.fgMuted)
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(tab == t ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 6))
                }.buttonStyle(.plain)
            }
            Spacer()
        }
        .padding(.horizontal, 12).padding(.vertical, 6)
        .background(Theme.bgSoft)
    }

    @ViewBuilder private var tabContent: some View {
        switch tab {
        case .overview:     overview
        case .files:        filesTab
        case .commits:      commitsTab
        case .checks:       checksTab
        case .conversation: conversationTab
        }
    }

    @ViewBuilder private var commitsTab: some View {
        Group {
            if let commits {
                if commits.isEmpty { centered("No commits since base") }
                else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(commits.enumerated()), id: \.element.id) { i, c in
                                commitRow(c, isFirst: i == 0, isLast: i == commits.count - 1)
                            }
                        }.padding(.vertical, 8)
                    }
                }
            } else { centered("loading commits…") }
        }
        .task(id: item.repo) { await loadCommits() }
    }

    private func commitRow(_ c: PRCommit, isFirst: Bool, isLast: Bool) -> some View {
        let nodeTop: CGFloat = 13
        return HStack(alignment: .top, spacing: 12) {
            ZStack(alignment: .top) {
                VStack(spacing: 0) {
                    Rectangle().fill(isFirst ? Color.clear : Theme.borderSoft).frame(width: 1.5, height: nodeTop + 5)
                    Rectangle().fill(isLast ? Color.clear : Theme.borderSoft).frame(width: 1.5).frame(maxHeight: .infinity)
                }
                Circle().fill(Theme.bg).overlay(Circle().stroke(Theme.accent, lineWidth: 2))
                    .frame(width: 10, height: 10).padding(.top, nodeTop)
            }
            .frame(width: 12).frame(maxHeight: .infinity)
            VStack(alignment: .leading, spacing: 3) {
                Text(c.subject).font(.system(size: 12.5)).foregroundStyle(Theme.fg).lineLimit(2)
                HStack(spacing: 8) {
                    Text(c.hash).font(Theme.mono(10.5)).foregroundStyle(Theme.accent)
                    if !c.author.isEmpty { Text(c.author).font(.system(size: 10.5)).foregroundStyle(Theme.fgMuted) }
                    if !c.date.isEmpty { Text("· \(c.date)").font(.system(size: 10.5)).foregroundStyle(Theme.dim) }
                }
            }
            .padding(.vertical, 8)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .fixedSize(horizontal: false, vertical: true)
    }

    @ViewBuilder private var overview: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if let rv = pr?.reviewers, !rv.isEmpty { reviewers(rv) }
                if let labels = pr?.labels, !labels.isEmpty { FlowChips(labels: labels) }
                if (pr?.reviewers.isEmpty == false) || (pr?.labels?.isEmpty == false) {
                    Rectangle().fill(Theme.borderSoft).frame(height: 1)
                }
                if let body = pr?.body, !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    MarkdownText(body)
                } else {
                    Text("No description.").font(.system(size: 12)).foregroundStyle(Theme.dim)
                }
            }
            .padding(16)
        }
    }

    private func reviewers(_ list: [PRInfo.Reviewer]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("REVIEWERS").font(.system(size: 10, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
            ForEach(list) { r in
                HStack(spacing: 8) {
                    Image(systemName: reviewerIcon(r.state)).font(.system(size: 12)).foregroundStyle(reviewerColor(r.state)).frame(width: 16)
                    Text(r.name).font(.system(size: 12.5)).foregroundStyle(Theme.fg)
                    Spacer()
                    Text(r.state).font(.system(size: 10, weight: .semibold)).foregroundStyle(reviewerColor(r.state))
                        .padding(.horizontal, 6).padding(.vertical, 1).background(reviewerColor(r.state).opacity(0.15), in: Capsule())
                }
            }
        }
    }
    private func reviewerIcon(_ s: String) -> String {
        switch s { case "approved": return "checkmark.seal.fill"; case "changes": return "exclamationmark.triangle.fill"
        case "commented": return "text.bubble.fill"; default: return "clock.fill" }
    }
    private func reviewerColor(_ s: String) -> Color {
        switch s { case "approved": return Theme.ok; case "changes": return Theme.danger
        case "commented": return Theme.fgMuted; default: return Theme.warn }
    }

    @ViewBuilder private var filesTab: some View {
        DiffFilesView(files: diffFiles, selFile: $selFile, filesTreeVisible: $filesTreeVisible, splitDiff: $splitDiff,
                      loadingLabel: "loading diff…", emptyLabel: "No file changes")
            .task(id: item.repo) { await loadDiff() }
    }

    @ViewBuilder private var checksTab: some View {
        if let checks = pr?.statusCheckRollup, !checks.isEmpty {
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(checks) { c in checkRow(c) }
                }.padding(12)
            }
        } else { centered("No checks") }
    }

    private struct Entry: Identifiable {
        let id: String; let author: String?; let body: String
        var tag: (String, Color)? = nil
        var context: String? = nil    // "path:line" for inline review comments
        var hunk: String? = nil
        var eventOnly: Bool = false
        let at: String
    }
    private var timeline: [Entry] {
        guard let pr else { return [] }
        var out: [Entry] = []
        if let b = pr.body, !b.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            out.append(Entry(id: "body", author: pr.author?.login, body: b, at: ""))
        }
        for r in pr.reviews ?? [] {
            let body = r.body ?? ""
            if body.isEmpty {
                if let tag = reviewTag(r.state) {
                    out.append(Entry(id: r.id, author: r.author?.login, body: "\(tag.0) these changes",
                                     tag: tag, eventOnly: true, at: r.submittedAt ?? ""))
                }
            } else {
                out.append(Entry(id: r.id, author: r.author?.login, body: body, tag: reviewTag(r.state), at: r.submittedAt ?? ""))
            }
        }
        for c in pr.comments ?? [] where !(c.body ?? "").isEmpty {
            out.append(Entry(id: c.id, author: c.author?.login, body: c.body ?? "", at: c.createdAt ?? ""))
        }
        for rc in reviewComments ?? [] where !(rc.body ?? "").isEmpty {
            let ctx = rc.path.map { "\($0)\(rc.line.map { ":\($0)" } ?? "")" }
            out.append(Entry(id: rc.id, author: rc.user, body: rc.body ?? "", context: ctx, hunk: rc.diffHunk, at: rc.createdAt ?? ""))
        }
        return out.sorted { ($0.id == "body" ? "" : $0.at) < ($1.id == "body" ? "" : $1.at) }
    }

    @ViewBuilder private var conversationTab: some View {
        let entries = timeline
        Group {
            if entries.isEmpty {
                centered(loadingDetail ? "loading…" : "No conversation")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        ForEach(entries) { e in entryView(e) }
                    }.padding(12)
                }
            }
        }
        .task(id: item.repo) { await loadReviewComments() }
    }

    @ViewBuilder private func entryView(_ e: Entry) -> some View {
        if e.eventOnly {
            HStack(spacing: 7) {
                if let (_, c) = e.tag { Circle().fill(c).frame(width: 7, height: 7) }
                Text(e.author ?? "someone").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.fg)
                Text(e.body).font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                Spacer()
            }.padding(.horizontal, 6).padding(.vertical, 2)
        } else if e.context != nil {
            commentBlock(author: e.author, body: e.body, tag: e.tag, context: e.context, hunk: e.hunk)
                .padding(.leading, 18)
        } else {
            commentBlock(author: e.author, body: e.body, tag: e.tag)
        }
    }


    private func checkRow(_ c: PRCheck) -> some View {
        let (icon, col): (String, Color) = {
            switch c.result {
            case .pass:    return ("checkmark.circle.fill", Theme.ok)
            case .fail:    return ("xmark.circle.fill", Theme.danger)
            case .pending: return ("clock.fill", Theme.warn)
            case .none:    return ("minus.circle", Theme.dim)
            }
        }()
        return Button { if let l = c.link, let u = URL(string: l) { NSWorkspace.shared.open(u) } } label: {
            HStack(spacing: 8) {
                Image(systemName: icon).font(.system(size: 12)).foregroundStyle(col)
                Text(c.label).font(.system(size: 12)).foregroundStyle(Theme.fg).lineLimit(1)
                Spacer()
                if c.link != nil { Image(systemName: "arrow.up.forward").font(.system(size: 9)).foregroundStyle(Theme.dim) }
            }
            .padding(.horizontal, 10).padding(.vertical, 7)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
            .contentShape(Rectangle())
        }.buttonStyle(.plain)
    }

    private func commentBlock(author: String?, body: String, tag: (String, Color)?, context: String? = nil, hunk: String? = nil) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text(author ?? "unknown").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.fg)
                if let (t, c) = tag {
                    Text(t).font(.system(size: 9.5, weight: .semibold)).foregroundStyle(c)
                        .padding(.horizontal, 5).padding(.vertical, 1).background(c.opacity(0.15), in: Capsule())
                }
                Spacer()
            }
            if let context {
                Text(context).font(Theme.mono(10)).foregroundStyle(Theme.accent).lineLimit(1)
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .background(Theme.accent.opacity(0.1), in: RoundedRectangle(cornerRadius: 5))
            }
            if let hunk, !hunk.isEmpty { hunkSnippet(hunk) }
            MarkdownText(body)
        }
        .padding(10)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
    }

    private func hunkSnippet(_ hunk: String) -> some View {
        let lines = Array(hunk.split(separator: "\n", omittingEmptySubsequences: false).map(String.init).suffix(6))
        return VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(lines.enumerated()), id: \.offset) { _, l in
                Text(l.isEmpty ? " " : l).font(Theme.mono(10.5))
                    .foregroundStyle(l.hasPrefix("+") ? Theme.ok : l.hasPrefix("-") ? Theme.danger : l.hasPrefix("@@") ? Theme.accent : Theme.fgSoft)
                    .lineLimit(1).truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 6)
                    .background(l.hasPrefix("+") ? Theme.ok.opacity(0.08) : l.hasPrefix("-") ? Theme.danger.opacity(0.08) : .clear)
            }
        }
        .padding(.vertical, 4)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.borderSoft))
    }

    private func reviewTag(_ state: String?) -> (String, Color)? {
        switch (state ?? "").uppercased() {
        case "APPROVED": return ("approved", Theme.ok)
        case "CHANGES_REQUESTED": return ("changes", Theme.warn)
        case "COMMENTED": return ("review", Theme.fgMuted)
        default: return nil
        }
    }

    private func statePill(_ pr: PRInfo) -> some View {
        let (txt, col): (String, Color) = pr.isDraft ? ("draft", Theme.dim)
            : pr.state == "MERGED" ? ("merged", Color(hex: 0xa371f7))
            : pr.state == "CLOSED" ? ("closed", Theme.danger)
            : ("open", Theme.ok)
        return Text(txt).font(.system(size: 10, weight: .semibold)).foregroundStyle(col)
            .padding(.horizontal, 7).padding(.vertical, 2).background(col.opacity(0.15), in: Capsule())
    }

    private func centered(_ s: String) -> some View {
        Text(s).font(.system(size: 12)).foregroundStyle(Theme.dim)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func loadDetail() async {
        loadingDetail = true
        let repo = item.repo
        let fresh = await Task.detached(priority: .userInitiated) { () -> PRInfo? in
            let d = PRStore.detail(branch: branch, repo: repo, isMain: isMain)
            struct R: Decodable { let pr: PRInfo? }
            return (PomJSON.decode(R.self, from: d))?.pr
        }.value
        loadingDetail = false
        if let fresh { detail = fresh }
    }

    private func loadReviewComments() async {
        guard reviewComments == nil else { return }
        let repo = item.repo
        let fresh = await Task.detached(priority: .utility) { () -> [PRReviewComment] in
            let d = PRStore.comments(branch: branch, repo: repo, isMain: isMain)
            struct R: Decodable { let comments: [PRReviewComment]? }
            return (PomJSON.decode(R.self, from: d))?.comments ?? []
        }.value
        reviewComments = fresh
    }

    private func loadCommits() async {
        guard commits == nil else { return }
        let repo = item.repo
        let fresh = await Task.detached(priority: .userInitiated) { () -> [PRCommit] in
            let d = PRStore.commits(branch: branch, repo: repo, isMain: isMain)
            struct R: Decodable { let commits: [PRCommit]? }
            return (PomJSON.decode(R.self, from: d))?.commits ?? []
        }.value
        commits = fresh
    }

    private func loadDiff() async {
        guard diffFiles == nil else { return }
        let repo = item.repo
        let files = await Task.detached(priority: .userInitiated) { () -> [DiffFile] in
            PomJSON.decode([DiffFile].self, from: PRStore.diff(branch: branch, repo: repo, isMain: isMain)) ?? []
        }.value
        diffFiles = files
        if selFile == nil { selFile = files.first?.path }
    }
}

struct FlowChips: View {
    @EnvironmentObject var theme: ThemeManager
    let labels: [PRInfo.Label]
    var body: some View {
        HStack(spacing: 6) {
            ForEach(labels) { l in
                let c = labelColor(l.color)
                Text(l.name ?? "").font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(c)
                    .padding(.horizontal, 8).padding(.vertical, 2)
                    .background(c.opacity(0.15), in: Capsule())
                    .overlay(Capsule().strokeBorder(c.opacity(0.4)))
            }
            Spacer(minLength: 0)
        }
    }
    private func labelColor(_ hex: String?) -> Color {
        guard let h = hex, let v = UInt32(h.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16) else { return Theme.fgMuted }
        return Color(hex: v)
    }
}
