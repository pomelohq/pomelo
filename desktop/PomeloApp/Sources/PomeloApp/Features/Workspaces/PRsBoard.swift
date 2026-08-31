import SwiftUI

struct WorkspacePR: Decodable, Identifiable, Equatable {
    let repo: String
    let alias: String
    var behind: Int = 0
    var ahead: Int = 0
    let pr: PRInfo?
    var id: String { repo }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        repo = try c.decode(String.self, forKey: .repo)
        alias = try c.decode(String.self, forKey: .alias)
        behind = try c.decodeIfPresent(Int.self, forKey: .behind) ?? 0
        ahead = try c.decodeIfPresent(Int.self, forKey: .ahead) ?? 0
        pr = try c.decodeIfPresent(PRInfo.self, forKey: .pr)
    }
    enum K: String, CodingKey { case repo, alias, behind, ahead, pr }
}

struct LocalChangeRepo: Decodable, Identifiable, Equatable {
    let repo: String
    let alias: String
    var files: Int = 0
    var insertions: Int = 0
    var deletions: Int = 0
    var behind: Int = 0
    var id: String { repo }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        repo = try c.decode(String.self, forKey: .repo)
        alias = try c.decode(String.self, forKey: .alias)
        files = try c.decodeIfPresent(Int.self, forKey: .files) ?? 0
        insertions = try c.decodeIfPresent(Int.self, forKey: .insertions) ?? 0
        deletions = try c.decodeIfPresent(Int.self, forKey: .deletions) ?? 0
        behind = try c.decodeIfPresent(Int.self, forKey: .behind) ?? 0
    }
    enum K: String, CodingKey { case repo, alias, files, insertions, deletions, behind }
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
    var reviewLog: [ReviewEntry]?
    var reviewRequests: [ReviewRequest]?
    var comments: [Comment]?
    var labels: [Label]?
    var statusCheckRollup: [Check]?

    struct Author: Decodable, Equatable { var login: String?; var avatarUrl: String? }
    struct ReviewRequest: Decodable, Equatable { var login: String?; var name: String?; var slug: String? }

    struct Reviewer: Decodable, Identifiable, Equatable {
        var name = ""; var state = ""; var id: String { name }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
            state = try c.decodeIfPresent(String.self, forKey: .state) ?? ""
        }
        enum K: String, CodingKey { case name, state }
    }
    var reviewers: [Reviewer] = []
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
        var result: ChecksStatus = .none
        var id: String { (name ?? context ?? "check") + (conclusion ?? state ?? status ?? "") }
        var label: String { name ?? context ?? "check" }
        var link: String? { detailsUrl ?? targetUrl }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            name = try c.decodeIfPresent(String.self, forKey: .name)
            context = try c.decodeIfPresent(String.self, forKey: .context)
            conclusion = try c.decodeIfPresent(String.self, forKey: .conclusion)
            state = try c.decodeIfPresent(String.self, forKey: .state)
            status = try c.decodeIfPresent(String.self, forKey: .status)
            detailsUrl = try c.decodeIfPresent(String.self, forKey: .detailsUrl)
            targetUrl = try c.decodeIfPresent(String.self, forKey: .targetUrl)
            workflowName = try c.decodeIfPresent(String.self, forKey: .workflowName)
            result = try c.decodeIfPresent(ChecksStatus.self, forKey: .result) ?? .none
        }
        enum K: String, CodingKey {
            case name, context, conclusion, state, status, detailsUrl, targetUrl, workflowName, result
        }
    }
    struct Review: Decodable, Equatable, Identifiable {
        var author: Author?
        var state: String?
        var body: String?
        var submittedAt: String?
        var id: String { (author?.login ?? "?") + (state ?? "") + (submittedAt ?? "") + String((body ?? "").prefix(12)) }
    }
    struct ReviewEntry: Decodable, Equatable {
        var reviewId: Int
        var author: Author?
        var state: String?
        var body: String?
        var submittedAt: String?
    }
    struct Comment: Decodable, Equatable, Identifiable {
        var author: Author?
        var body: String?
        var createdAt: String?
        var id: String { (author?.login ?? "?") + String((body ?? "").prefix(24)) }
    }

    enum ChecksStatus: String, Decodable { case pass, fail, pending, none }
    enum ReviewDecision: String, Decodable { case approved, changes, review, none }

    var checks: ChecksStatus = .none
    var review: ReviewDecision = .none
    var conflict: Bool = false

    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        number = try c.decode(Int.self, forKey: .number)
        title = try c.decode(String.self, forKey: .title)
        state = try c.decode(String.self, forKey: .state)
        url = try c.decode(String.self, forKey: .url)
        isDraft = try c.decodeIfPresent(Bool.self, forKey: .isDraft) ?? false
        mergeable = try c.decodeIfPresent(String.self, forKey: .mergeable)
        reviewDecision = try c.decodeIfPresent(String.self, forKey: .reviewDecision)
        body = try c.decodeIfPresent(String.self, forKey: .body)
        headRefName = try c.decodeIfPresent(String.self, forKey: .headRefName)
        baseRefName = try c.decodeIfPresent(String.self, forKey: .baseRefName)
        additions = try c.decodeIfPresent(Int.self, forKey: .additions)
        deletions = try c.decodeIfPresent(Int.self, forKey: .deletions)
        changedFiles = try c.decodeIfPresent(Int.self, forKey: .changedFiles)
        author = try c.decodeIfPresent(Author.self, forKey: .author)
        reviews = try c.decodeIfPresent([Review].self, forKey: .reviews)
        reviewLog = try c.decodeIfPresent([ReviewEntry].self, forKey: .reviewLog)
        reviewRequests = try c.decodeIfPresent([ReviewRequest].self, forKey: .reviewRequests)
        comments = try c.decodeIfPresent([Comment].self, forKey: .comments)
        labels = try c.decodeIfPresent([Label].self, forKey: .labels)
        statusCheckRollup = try c.decodeIfPresent([Check].self, forKey: .statusCheckRollup)
        reviewers = try c.decodeIfPresent([Reviewer].self, forKey: .reviewers) ?? []
        checks = try c.decodeIfPresent(ChecksStatus.self, forKey: .checks) ?? .none
        review = try c.decodeIfPresent(ReviewDecision.self, forKey: .review) ?? .none
        conflict = try c.decodeIfPresent(Bool.self, forKey: .conflict) ?? false
    }
    enum K: String, CodingKey {
        case number, title, state, url, isDraft, mergeable, reviewDecision, body,
             headRefName, baseRefName, additions, deletions, changedFiles, author,
             reviews, reviewLog, reviewRequests, comments, labels, statusCheckRollup,
             reviewers, checks, review, conflict
    }
}

typealias PRCheck = PRInfo.Check
typealias PRReview = PRInfo.Review

struct PRCommit: Decodable, Identifiable, Equatable {
    let hash: String; let subject: String
    var author: String = ""; var date: String = ""
    var id: String { hash }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        hash = try c.decode(String.self, forKey: .hash)
        subject = try c.decode(String.self, forKey: .subject)
        author = try c.decodeIfPresent(String.self, forKey: .author) ?? ""
        date = try c.decodeIfPresent(String.self, forKey: .date) ?? ""
    }
    enum K: String, CodingKey { case hash, subject, author, date }
}

struct PRReviewComment: Decodable, Identifiable, Equatable {
    var user: String?; var avatarUrl: String?; var body: String?; var path: String?; var line: Int?
    var diffHunk: String?; var hunkLines: [DiffLine]?; var createdAt: String?; var reviewId: Int?
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
    // Narrow layout only: whether the detail column is showing instead of the list.
    @State private var drilled = false
    @State private var loading = true
    @State private var refreshing = false
    @State private var pollTask: Task<Void, Never>?
    @AppStorage("prs.masterWidth") private var masterWidth = 320.0
    @AppStorage("prs.sectionExpanded") private var sectionExpanded = true
    @AppStorage("prs.localSectionExpanded") private var localSectionExpanded = true

    var body: some View {
        GeometryReader { geo in
            let narrow = geo.size.width < PaneMetrics.stackWidth
            Group {
                if narrow { stacked } else { sideBySide(total: geo.size.width) }
            }
            .frame(width: geo.size.width, height: geo.size.height, alignment: .topLeading)
            .environment(\.paneNarrow, narrow)
        }
        .background(Theme.bg)
        .task(id: workspace.id) {
            if prs.isEmpty {
                let cached = state.prsFor(workspace.id)
                if !cached.isEmpty {
                    prs = cached; loading = false
                    if selection == nil { selection = .pr(cached[0].repo) }
                }
            }
            await load(); startPolling()
        }
        .onDisappear { pollTask?.cancel() }
    }

    // Wide: master list beside the detail, user-resizable. The master is clamped
    // against the real width so a dragged-narrow agent split can't starve the detail.
    private func sideBySide(total: CGFloat) -> some View {
        let w = min(masterWidth, Swift.max(220, Double(total) - PaneMetrics.minDetail))
        return HStack(spacing: 0) {
            master.frame(width: w)
            SplitHandle(axis: .horizontal, value: $masterWidth, min: 220,
                        max: Swift.max(220, Double(total) - PaneMetrics.minDetail))
            detail.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // Narrow: one column at a time. The list drills into the detail, which gets a
    // back button — same idiom as the commit-diff drill-down inside PRDetail.
    @ViewBuilder private var stacked: some View {
        if drilled, selection != nil {
            VStack(spacing: 0) {
                BackBar(title: selectionTitle) { drilled = false }
                Divider().overlay(Theme.borderSoft)
                detail.frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        } else {
            master.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    @ViewBuilder private var detail: some View {
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

    private var selectionTitle: String {
        switch selection {
        case .pr(let repo):    return prs.first { $0.repo == repo }?.alias ?? repo
        case .local(let repo): return localChanges.first { $0.repo == repo }?.alias ?? repo
        case nil:              return ""
        }
    }

    private func select(_ s: Selection) {
        selection = s
        drilled = true
    }

    private var master: some View {
        VStack(spacing: 0) {
            localChangesSection
            prSection
        }
        .background(Theme.bgSoft)
    }

    @ViewBuilder private var localChangesSection: some View {
        SectionHeader(title: "LOCAL CHANGES", expanded: $localSectionExpanded, count: localChanges.isEmpty ? nil : localChanges.count)
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
                            .onTapGesture { select(.local(item.repo)) }
                    }
                }
                .padding(8)
            }
        }
    }

    private var prSection: some View {
        VStack(spacing: 0) {
            SectionHeader(title: "PULL REQUESTS", expanded: $sectionExpanded, loading: loading) {
                if refreshing { Spinner(size: 11).frame(width: 16, height: 14) }
                else { IconButton("arrow.clockwise", size: 11, tip: "Refresh") { refresh() } }
            }
            Divider().overlay(Theme.borderSoft)

            if sectionExpanded {
                if prs.isEmpty && loading {
                    LoadingView(text: "loading pull requests…")
                } else if prs.isEmpty {
                    EmptyStateView(icon: "arrow.triangle.pull", title: "No pull requests")
                } else {
                    ScrollView {
                        LazyVStack(spacing: 4) {
                            ForEach(prs) { item in
                                PRRow(item: item, active: selection == .pr(item.repo))
                                    .contentShape(Rectangle())
                                    .onTapGesture { select(.pr(item.repo)) }
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

    private func refresh() {
        guard !refreshing else { return }
        refreshing = true
        let branch = workspace.branch, isMain = workspace.isMain
        Task {
            await Task.detached(priority: .userInitiated) { _ = PRStore.refresh(branch: branch, isMain: isMain) }.value
            await load()
            refreshing = false
        }
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

    // Wrapped, not clipped: a narrow master column can't fit draft+CI+review+conflict on one line.
    @ViewBuilder private func badges(_ pr: PRInfo) -> some View {
        WrapHStack(spacing: 5) {
            if pr.state == "MERGED" {
                mini("arrow.triangle.merge", "merged", Color(hex: 0xa371f7))   // terminal — CI/review no longer relevant
            } else if pr.state == "CLOSED" {
                mini("xmark", "closed", Theme.dim)
            } else {
                openBadges(pr)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
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
        }
    }
    private func mini(_ icon: String, _ text: String, _ c: Color) -> some View {
        Badge(icon: icon, text: text, color: c)
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
    @Environment(\.paneNarrow) private var narrow
    let item: LocalChangeRepo
    let branch: String
    let isMain: Bool

    @State private var diffFiles: [DiffFile]?
    @State private var selFile: String?
    @State private var splitDiff = CodeDisplayManager.shared.defaultSplit
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
            StatusPill(text: "local", color: Theme.warn)
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
    @Environment(\.paneNarrow) private var narrow
    let item: WorkspacePR
    let branch: String
    let isMain: Bool

    enum Tab: String, CaseIterable { case overview = "Overview", files = "Files", commits = "Commits", checks = "Checks", conversation = "Conversation" }
    @State private var tab: Tab = .overview
    @State private var detail: PRInfo?
    @State private var diffFiles: [DiffFile]?
    @State private var selFile: String?
    @State private var splitDiff = CodeDisplayManager.shared.defaultSplit
    @AppStorage("prs.filesTreeVisible") private var filesTreeVisible = true
    @State private var commits: [PRCommit]?
    @State private var selCommit: String?
    @State private var commitFiles: [DiffFile]?
    @State private var selCommitFile: String?
    @State private var timelineItems: [PRTimelineItem] = []
    @State private var timelineLoaded = false
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
                // Author / refs / counts wrap rather than squeezing the branch pair to ellipsis.
                WrapHStack(spacing: 8, lineSpacing: 3) {
                    if let a = pr.author?.login { Label(a, systemImage: "person.crop.circle").labelStyle(.titleAndIcon).font(.system(size: 11)).foregroundStyle(Theme.fgMuted) }
                    if let h = pr.headRefName, let b = pr.baseRefName {
                        Text("\(h) → \(b)").font(Theme.mono(10.5)).foregroundStyle(Theme.dim).lineLimit(1)
                    }
                    if let a = pr.additions, let d = pr.deletions {
                        Text("+\(a)").font(Theme.mono(11)).foregroundStyle(Theme.ok)
                        Text("-\(d)").font(Theme.mono(11)).foregroundStyle(Theme.danger)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, narrow ? 12 : 16).padding(.vertical, 12)
        } else {
            Text(loadingDetail ? "loading…" : "no open PR").font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
                .padding(16)
        }
    }

    // Five tabs don't fit a narrow detail column, so let them scroll instead of clip.
    private var tabBar: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 2) {
                SegmentedTabs(tabs: Tab.allCases.map { $0 }, selection: $tab, label: { $0.rawValue }, accent: true)
                Spacer()
            }
            ScrollView(.horizontal, showsIndicators: false) {
                SegmentedTabs(tabs: Tab.allCases.map { $0 }, selection: $tab, label: { $0.rawValue }, accent: true)
            }
        }
        .padding(.horizontal, narrow ? 8 : 12).padding(.vertical, 6)
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
            if let sel = selCommit, let c = commits?.first(where: { $0.id == sel }) {
                commitDiffView(c)
            } else if let commits {
                if commits.isEmpty { EmptyStateView(icon: "circle.dashed", title: "No commits since base") }
                else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(commits.enumerated()), id: \.element.id) { i, c in
                                Button { selCommit = c.id } label: {
                                    commitRow(c, isFirst: i == 0, isLast: i == commits.count - 1)
                                }
                                .buttonStyle(.plain)
                            }
                        }.padding(.vertical, 8).readingColumn()
                    }
                }
            } else { LoadingView(text: "loading commits…") }
        }
        .task(id: item.repo) { await loadCommits() }
        .task(id: selCommit) { await loadCommitDiff() }
    }

    private func commitDiffView(_ c: PRCommit) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Button { selCommit = nil } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "chevron.left").font(.system(size: 10, weight: .semibold))
                        Text("Commits").font(.system(size: 11.5))
                    }.foregroundStyle(Theme.accent)
                }
                .buttonStyle(.plain).keyboardShortcut(.escape, modifiers: [])
                Text(c.hash).font(Theme.mono(10.5)).foregroundStyle(Theme.accent)
                Text(c.subject).font(.system(size: 12)).foregroundStyle(Theme.fg)
                    .lineLimit(1).truncationMode(.tail)
                Spacer(minLength: 0)
                if !c.author.isEmpty { Text(c.author).font(.system(size: 10.5)).foregroundStyle(Theme.fgMuted) }
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .background(Theme.bgSoft)
            Divider().overlay(Theme.borderSoft)
            DiffFilesView(files: commitFiles, selFile: $selCommitFile, filesTreeVisible: $filesTreeVisible,
                          splitDiff: $splitDiff, loadingLabel: "loading commit diff…",
                          emptyLabel: "This commit changed nothing")
        }
    }

    private func loadCommitDiff() async {
        guard let sha = selCommit else { commitFiles = nil; return }
        commitFiles = nil
        let repo = item.repo
        let files = await Task.detached(priority: .userInitiated) { () -> [DiffFile] in
            PomJSON.decode([DiffFile].self, from: PRStore.commitDiff(branch: branch, repo: repo, sha: sha, isMain: isMain)) ?? []
        }.value
        guard selCommit == sha else { return }
        commitFiles = files
        selCommitFile = files.first?.path
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
                    commentCard(author: pr?.author?.login, avatar: pr?.author?.avatarUrl, headline: "opened this pull request", body: body)
                } else {
                    Text("No description.").font(.system(size: 12)).foregroundStyle(Theme.dim)
                }
            }
            .padding(16)
            .readingColumn()
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
                    StatusPill(text: r.state, color: reviewerColor(r.state), uppercase: true)
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
                }.padding(12).readingColumn()
            }
        } else if loadingDetail && detail == nil {
            LoadingView(text: "loading checks…")
        } else { EmptyStateView(icon: "checklist", title: "No checks") }
    }

    @ViewBuilder private var conversationTab: some View {
        Group {
            if timelineItems.isEmpty {
                if timelineLoaded { EmptyStateView(icon: "bubble.left.and.bubble.right", title: "No conversation") }
                else { LoadingView(text: "loading conversation…") }
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(timelineItems.enumerated()), id: \.element.id) { i, it in
                            timelineRow(it, isLast: i == timelineItems.count - 1)
                        }
                    }
                    .padding(.horizontal, narrow ? 10 : 14).padding(.vertical, 12)
                    .readingColumn(940)
                }
            }
        }
        .task(id: item.repo) { await loadTimeline() }
        .onChange(of: detail) { Task { await loadTimeline() } }
    }

    // Avatar + rail is 72pt of chrome before a single word of comment; narrow drops
    // the rail and shrinks the avatar so the text keeps a readable measure.
    private var railWidth: CGFloat { narrow ? 0 : 28 }
    private var avatarSize: CGFloat { narrow ? 24 : 36 }

    @ViewBuilder private func timelineRow(_ it: PRTimelineItem, isLast: Bool) -> some View {
        HStack(alignment: .top, spacing: 10) {
            ZStack(alignment: .top) {
                VStack(spacing: 0) {
                    Color.clear.frame(height: avatarSize)
                    if !isLast { Rectangle().fill(Theme.borderSoft).frame(width: 2).frame(maxHeight: .infinity) }
                }
                avatarBadge(it)
            }
            .frame(width: avatarSize)
            .frame(maxHeight: .infinity, alignment: .top)
            VStack(alignment: .leading, spacing: 8) {
                switch it.kind {
                case "description":
                    commentCard(author: it.author, avatar: it.avatar, headline: "opened this pull request", body: it.body)
                case "comment":
                    commentCard(author: it.author, avatar: it.avatar, headline: "commented", body: it.body)
                case "review":
                    HStack(spacing: 6) {
                        Text(it.author ?? "someone").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.fg)
                        Text(reviewVerb(it.state)).font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                        Spacer()
                    }.frame(height: avatarSize)
                    if !it.body.isEmpty { commentCard(author: it.author, avatar: it.avatar, headline: "left a comment", body: it.body) }
                    ForEach(it.inline) { rc in inlineCard(rc).padding(.leading, narrow ? 0 : 24) }
                default:
                    if let rc = it.inline.first { inlineCard(rc) }
                }
            }
            .padding(.bottom, 16)
        }
    }

    // Avatar sits on the thread rail; review state shows as a small corner badge.
    @ViewBuilder private func avatarBadge(_ it: PRTimelineItem) -> some View {
        ZStack(alignment: .bottomTrailing) {
            Avatar(url: it.avatar, name: it.author, size: avatarSize)
            if it.kind == "review" || it.kind == "inline" {
                let (icon, col) = reviewBadge(it.state)
                Image(systemName: icon).font(.system(size: 9, weight: .bold)).foregroundStyle(col)
                    .padding(3).background(Theme.bg, in: Circle())
                    .overlay(Circle().strokeBorder(Theme.bg, lineWidth: 1.5))
                    .offset(x: 3, y: 3)
            }
        }
    }

    private func reviewBadge(_ state: String) -> (String, Color) {
        switch state {
        case "APPROVED": return ("checkmark", Theme.ok)
        case "CHANGES_REQUESTED": return ("xmark", Theme.danger)
        default: return ("eye", Theme.fgMuted)
        }
    }

    private func reviewVerb(_ state: String) -> String {
        switch state {
        case "APPROVED": return "approved these changes"
        case "CHANGES_REQUESTED": return "requested changes"
        case "DISMISSED": return "review dismissed"
        default: return "reviewed"
        }
    }

    private func commentCard(author: String?, avatar: String? = nil, headline: String, body: String) -> some View {
        Card {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Avatar(url: avatar, name: author, size: 20)
                    Text(author ?? "unknown").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text(headline).font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
                    Spacer()
                }
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(Theme.panel3)
                Divider().overlay(Theme.borderSoft)
                MarkdownText(body).padding(12)
            }
        }
    }

    private func inlineCard(_ rc: PRReviewComment) -> some View {
        Card {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Image(systemName: "doc.text").font(.system(size: 10)).foregroundStyle(Theme.fgMuted)
                    Text(rc.path.map { "\($0)\(rc.line.map { ":\($0)" } ?? "")" } ?? "")
                        .font(Theme.mono(10.5)).foregroundStyle(Theme.fg).lineLimit(1).truncationMode(.middle)
                    Spacer()
                }
                .padding(.horizontal, 12).padding(.vertical, 7)
                .background(Theme.panel3)
                if let lines = rc.hunkLines, !lines.isEmpty {
                    Divider().overlay(Theme.borderSoft)
                    hunkSnippet(lines)
                }
                Divider().overlay(Theme.borderSoft)
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 6) {
                        Avatar(url: rc.avatarUrl, name: rc.user, size: 24)
                        Text(rc.user ?? "unknown").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.fg)
                    }
                    MarkdownText(rc.body ?? "")
                }
                .padding(12)
            }
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
            Card {
                HStack(spacing: 8) {
                    Image(systemName: icon).font(.system(size: 12)).foregroundStyle(col)
                    Text(c.label).font(.system(size: 12)).foregroundStyle(Theme.fg).lineLimit(1)
                    Spacer()
                    if c.link != nil { Image(systemName: "arrow.up.forward").font(.system(size: 9)).foregroundStyle(Theme.dim) }
                }
                .padding(.horizontal, 10).padding(.vertical, 7)
            }
            .contentShape(Rectangle())
        }.buttonStyle(.plain)
    }

    private func hunkSnippet(_ lines: [DiffLine]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(lines.suffix(6)) { l in
                let tint: Color? = l.kind == .add ? Theme.ok : l.kind == .del ? Theme.danger : nil
                let num = l.kind == .del ? l.oldN : l.newN
                HStack(spacing: 0) {
                    Text(num.map(String.init) ?? "").font(Theme.mono(10)).foregroundStyle(Theme.dim)
                        .frame(width: 36, alignment: .trailing)
                    Text(l.kind == .add ? "+" : l.kind == .del ? "-" : " ").font(Theme.mono(10.5, .bold))
                        .foregroundStyle(tint ?? Theme.dim).frame(width: 16)
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
        return StatusPill(text: txt, color: col)
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

    private func loadTimeline() async {
        let repo = item.repo
        let fresh = await Task.detached(priority: .utility) { () -> [PRTimelineItem] in
            let d = PRStore.timeline(branch: branch, repo: repo, isMain: isMain)
            struct R: Decodable { let items: [PRTimelineItem]? }
            return (PomJSON.decode(R.self, from: d))?.items ?? []
        }.value
        if fresh != timelineItems { timelineItems = fresh }
        timelineLoaded = true
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
