import SwiftUI

struct DashboardView: View {
    @StateObject private var vm: DashboardViewModel
    @EnvironmentObject var theme: ThemeManager
    let client: RemoteClient
    @State private var creating = false
    @State private var createBusy = false
    @State private var createError = ""
    @State private var repoNames: [String] = []
    @State private var picked: Set<String> = []
    @State private var dispName = ""
    @State private var slug = ""
    @State private var desc = ""
    @State private var refining = false
    @State private var boards: [JiraBoard] = []
    @State private var board: JiraBoard?
    @State private var sprint: [SprintIssue] = []
    @State private var loadingSprint = false
    @State private var pickedKey = ""
    @State private var boardMenuOpen = false
    @State private var renaming: WorkspaceRow?
    @State private var renameText = ""
    @State private var renameBusy = false

    init(client: RemoteClient) {
        self.client = client
        _vm = StateObject(wrappedValue: DashboardViewModel(client: client))
    }

    private var runningCount: Int { vm.workspaces.filter { $0.running > 0 }.count }
    private var agentCount: Int { vm.workspaces.filter { agentOrbActive(vm.agentState($0)) }.count }

    private let columns = [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)]

    private var mainWs: WorkspaceRow? { vm.workspaces.first { $0.isMain } }
    private var others: [WorkspaceRow] { vm.workspaces.filter { !$0.isMain } }

    private var headerSubtitle: String {
        var parts = ["\(vm.workspaces.count) workspaces"]
        if runningCount > 0 { parts.append("\(runningCount) running") }
        if agentCount > 0 { parts.append("\(agentCount) agents") }
        return parts.joined(separator: "  ·  ")
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 10) {
                if !vm.reachable {
                    HStack(spacing: 6) {
                        Image(systemName: "wifi.exclamationmark")
                        Text(vm.lastError.isEmpty ? "Can't reach this Mac" : vm.lastError)
                    }
                    .font(Theme.ui(12)).foregroundStyle(Theme.warn)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                }
                if let m = mainWs {
                    NavigationLink { WorkspaceDetailView(client: client, workspace: m) } label: { wideCard(m) }
                        .buttonStyle(.plain)
                }
                LazyVGrid(columns: columns, spacing: 10) {
                    ForEach(others) { ws in
                        NavigationLink { WorkspaceDetailView(client: client, workspace: ws) } label: { card(ws) }
                            .buttonStyle(.plain)
                            .contextMenu {
                                Button { renameText = ws.displayName; renaming = ws } label: {
                                    Label("Rename", systemImage: "pencil")
                                }
                            }
                    }
                }
            }
            .padding(12)
        }
        .background(Theme.bg.ignoresSafeArea())
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(client.device.name.isEmpty ? client.device.host : client.device.name)
                        .font(Theme.ui(16, .semibold)).foregroundStyle(Theme.fg)
                    Text(headerSubtitle)
                        .font(Theme.ui(11)).foregroundStyle(Theme.fgMuted)
                }
            }
            ToolbarItem(placement: .primaryAction) {
                Button { resetCreate(); creating = true } label: {
                    Image(systemName: "plus").foregroundStyle(Theme.accent)
                }
            }
        }
        .toolbarBackground(Theme.bgSoft, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbarColorScheme(.dark, for: .navigationBar)
        .task { await vm.pollLoop() }
        .refreshable { await vm.load() }
        .sheet(isPresented: $creating) { createSheet }
        .sheet(item: $renaming) { ws in renameSheet(ws) }
    }

    private func renameSheet(_ ws: WorkspaceRow) -> some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 14) {
                SectionLabel(text: "Display name")
                HStack(spacing: 8) {
                    fieldBG {
                        TextField("Display name", text: $renameText)
                            .font(Theme.ui(14)).foregroundStyle(Theme.fg)
                    }
                    Button { refineRename(ws) } label: {
                        HStack(spacing: 4) {
                            Text(renameBusy ? "..." : "Refine")
                        }
                        .font(Theme.ui(12, .semibold)).foregroundStyle(Theme.accent)
                        .padding(.horizontal, 10).padding(.vertical, 9)
                        .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 10))
                    }.disabled(renameBusy)
                }
                Text("Branch: \(ws.branch)").font(Theme.mono(10)).foregroundStyle(Theme.fgMuted).lineLimit(1)
                Text("Refine asks Claude for a tidy display name from the branch.")
                    .font(Theme.ui(10.5)).foregroundStyle(Theme.fgMuted)
                Spacer()
            }
            .padding(16)
            .background(Theme.bg.ignoresSafeArea())
            .navigationTitle("Rename workspace")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSoft, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { renaming = nil } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") {
                        let name = renameText.trimmingCharacters(in: .whitespaces)
                        renaming = nil
                        Task {
                            _ = try? await client.renameWorkspace(branch: ws.branch, isMain: ws.isMain, displayName: name)
                            await vm.load()
                        }
                    }
                }
            }
        }
        .presentationDetents([.height(240)])
        .preferredColorScheme(activeThemeMode == .light ? .light : .dark)
    }

    private func refineRename(_ ws: WorkspaceRow) {
        renameBusy = true
        Task {
            struct R: Decodable { var name = "" }
            let seed = renameText.trimmingCharacters(in: .whitespaces).isEmpty ? ws.branch : renameText
            if let d = try? await client.suggestName(branch: ws.branch, desc: seed),
               let r = PomJSON.decode(R.self, from: d), !r.name.isEmpty {
                renameText = r.name
            }
            renameBusy = false
        }
    }

    private func fieldBG<C: View>(@ViewBuilder _ c: () -> C) -> some View {
        c().padding(10)
            .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderSoft, lineWidth: 1))
    }

    @ViewBuilder private var describeSection: some View {
        section("Describe the work") {
            HStack(spacing: 8) {
                fieldBG {
                    TextField("Jira key or a short description...", text: $desc)
                        .font(Theme.ui(13)).foregroundStyle(Theme.fg)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                }
                Button { refine() } label: {
                    HStack(spacing: 4) {
                        Text(refining ? "..." : "Refine")
                    }
                    .font(Theme.ui(12, .semibold)).foregroundStyle(Theme.accent)
                    .padding(.horizontal, 10).padding(.vertical, 9)
                    .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 10))
                }.disabled(desc.trimmingCharacters(in: .whitespaces).isEmpty || refining)
            }
            Text("Type a Jira key (PROJ-101) or describe it, then Refine to get a name + branch from Claude.")
                .font(Theme.ui(10.5)).foregroundStyle(Theme.fgMuted)
        }
    }

    @ViewBuilder private var nameSection: some View {
        section("Name") {
            fieldBG {
                TextField("Display name (optional)", text: $dispName)
                    .font(Theme.ui(14)).foregroundStyle(Theme.fg)
            }
        }
    }

    @ViewBuilder private var branchSection: some View {
        section("Branch (slug)") {
            fieldBG {
                TextField("feat-login or proj-101-...", text: $slug)
                    .font(Theme.mono(13)).foregroundStyle(Theme.fg)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
            }
        }
    }

    @ViewBuilder private var jiraSection: some View {
        section("Jira ticket (optional)") {
            boardDropdown
            if loadingSprint {
                loadingRow("loading sprint...")
            } else {
                let list = Array(filteredSprint.prefix(40))
                if list.isEmpty {
                    Text(desc.isEmpty ? "No tickets in this sprint." : "No tickets match \"\(desc)\".")
                        .font(Theme.ui(11)).foregroundStyle(Theme.fgMuted)
                } else {
                    ScrollView {
                        VStack(spacing: 0) {
                            ForEach(list) { iss in
                                Button { pickTicket(iss) } label: { ticketRow(iss) }.buttonStyle(.plain)
                                if iss.id != list.last?.id { Divider().overlay(Theme.borderSoft) }
                            }
                        }
                        .padding(.horizontal, 12)
                    }
                    .frame(maxHeight: 300)
                    .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderSoft, lineWidth: 1))
                }
            }
        }
    }

    @ViewBuilder private var boardDropdown: some View {
        Button { boardMenuOpen = true } label: {
            HStack(spacing: 7) {
                Circle().fill(Theme.warn).frame(width: 6, height: 6)
                Text(board?.name ?? "Select board").font(Theme.mono(12, .medium)).foregroundStyle(Theme.fg)
                Spacer()
                Image(systemName: "chevron.down").font(.system(size: 10, weight: .semibold)).foregroundStyle(Theme.fgMuted)
            }
            .padding(11)
            .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(boardMenuOpen ? Theme.accent.opacity(0.5) : Theme.borderSoft, lineWidth: 1))
        }
        .buttonStyle(.plain)
        .popover(isPresented: $boardMenuOpen) {
            ScrollView {
                VStack(spacing: 0) {
                    ForEach(boards) { b in
                        let on = board?.id == b.id
                        Button {
                            board = b
                            boardMenuOpen = false
                            Task { await loadSprint() }
                        } label: {
                            HStack(spacing: 8) {
                                Text(b.name).font(Theme.mono(12.5, on ? .semibold : .regular))
                                    .foregroundStyle(on ? Theme.accent : Theme.fg)
                                Spacer()
                                if on { Image(systemName: "checkmark").font(.system(size: 11, weight: .bold)).foregroundStyle(Theme.accent) }
                            }
                            .padding(.horizontal, 16).padding(.vertical, 12).contentShape(Rectangle())
                        }.buttonStyle(.plain)
                        if b.id != boards.last?.id { Divider().overlay(Theme.borderSoft) }
                    }
                }
            }
            .frame(width: 280, height: min(CGFloat(boards.count) * 45 + 8, 420))
            .presentationCompactAdaptation(.popover)
            .presentationBackground(Theme.panel3)
        }
    }

    @ViewBuilder private var reposSection: some View {
        section("Repos") {
            if repoNames.isEmpty {
                loadingRow("loading repos...")
            } else {
                VStack(spacing: 0) {
                    ForEach(repoNames, id: \.self) { r in
                        Button { toggleRepo(r) } label: {
                            HStack(spacing: 10) {
                                Image(systemName: picked.contains(r) ? "checkmark.square.fill" : "square")
                                    .foregroundStyle(picked.contains(r) ? Theme.accent : Theme.dim)
                                Text(r).font(Theme.mono(12.5)).foregroundStyle(Theme.fg)
                                Spacer()
                            }.padding(.vertical, 9).contentShape(Rectangle())
                        }.buttonStyle(.plain)
                        if r != repoNames.last { Divider().overlay(Theme.borderSoft) }
                    }
                }
                .padding(.horizontal, 12)
                .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderSoft, lineWidth: 1))
                Text("Empty = default combo (all repos).").font(Theme.ui(10.5)).foregroundStyle(Theme.fgMuted)
            }
        }
    }

    private var createSheet: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    describeSection
                    if !boards.isEmpty { jiraSection }
                    nameSection
                    branchSection
                    reposSection
                    if !createError.isEmpty {
                        Text(createError).font(Theme.ui(11)).foregroundStyle(Theme.danger)
                    }
                }
                .padding(16)
            }
            .background(Theme.bg.ignoresSafeArea())
            .navigationTitle("Create Workspace")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSoft, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { creating = false } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(createBusy ? "Creating..." : "Create") { create() }
                        .disabled(slug.trimmingCharacters(in: .whitespaces).isEmpty || createBusy)
                }
            }
            .task { await loadRepos(); await loadJira() }
        }
        .preferredColorScheme(activeThemeMode == .light ? .light : .dark)
    }

    private var filteredSprint: [SprintIssue] {
        let q = desc.trimmingCharacters(in: .whitespaces).uppercased()
        if q.isEmpty { return sprint.sorted { $0.mine && !$1.mine } }
        func score(_ i: SprintIssue) -> Int {
            let k = i.key.uppercased(), s = i.summary.uppercased()
            if k.hasPrefix(q) { return 100 }
            if k.contains(q) { return 60 }
            if s.contains(q) { return 30 }
            return 0
        }
        var scored: [(iss: SprintIssue, score: Int)] = []
        for i in sprint {
            let sc = score(i)
            if sc > 0 { scored.append((i, sc)) }
        }
        scored.sort { a, b in
            if a.iss.mine != b.iss.mine { return a.iss.mine }
            if a.score != b.score { return a.score > b.score }
            return a.iss.key < b.iss.key
        }
        return scored.map { $0.iss }
    }

    private func loadingRow(_ text: String) -> some View {
        HStack(spacing: 6) {
            ProgressView().controlSize(.small).tint(Theme.accent)
            Text(text).font(Theme.ui(11)).foregroundStyle(Theme.fgMuted)
        }
    }

    private func ticketRow(_ iss: SprintIssue) -> some View {
        let on = pickedKey == iss.key
        return HStack(alignment: .top, spacing: 9) {
            ticketAvatar(iss)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(iss.key).font(Theme.mono(11, .medium)).foregroundStyle(Theme.accent)
                    Text(iss.summary).font(Theme.ui(11.5)).foregroundStyle(Theme.fg).lineLimit(1)
                }
                HStack(spacing: 6) {
                    Text(iss.mine ? "You" : (iss.assignee.isEmpty ? "Unassigned" : iss.assignee))
                        .font(Theme.ui(10)).foregroundStyle(iss.mine ? Theme.accent : Theme.fgMuted).lineLimit(1)
                    if !iss.sprint.isEmpty { ticketBadge(iss.sprint) }
                    if !iss.status.isEmpty { ticketBadge(iss.status) }
                }
            }
            Spacer(minLength: 6)
            if on {
                Image(systemName: "checkmark.circle.fill").font(.system(size: 14)).foregroundStyle(Theme.accent)
            } else if iss.mine {
                Text("mine").font(.system(size: 8.5, weight: .bold)).foregroundStyle(Theme.accent)
                    .padding(.horizontal, 5).padding(.vertical, 1).background(Theme.accentSoft, in: Capsule())
            }
        }
        .padding(.vertical, 8).contentShape(Rectangle())
    }

    private func ticketAvatar(_ iss: SprintIssue) -> some View {
        let name = iss.mine ? "You" : iss.assignee
        let initial = String(name.prefix(1)).uppercased()
        return ZStack {
            if let u = URL(string: iss.avatar), !iss.avatar.isEmpty {
                AsyncImage(url: u) { phase in
                    if let img = phase.image { img.resizable().scaledToFill() }
                    else { avatarFallback(initial, iss.mine) }
                }
            } else { avatarFallback(initial, iss.mine) }
        }
        .frame(width: 22, height: 22).clipShape(Circle()).padding(.top, 1)
    }

    private func avatarFallback(_ initial: String, _ mine: Bool) -> some View {
        Circle().fill(mine ? Theme.accent.opacity(0.25) : Theme.chip)
            .overlay(Text(initial.isEmpty ? "?" : initial).font(.system(size: 9, weight: .semibold))
                .foregroundStyle(mine ? Theme.accent : Theme.fgMuted))
    }

    private func ticketBadge(_ text: String) -> some View {
        Text(text).font(Theme.ui(9)).foregroundStyle(Theme.dim).lineLimit(1)
            .padding(.horizontal, 5).padding(.vertical, 1).background(Theme.chip, in: Capsule())
    }

    private func loadJira() async {
        guard boards.isEmpty else { return }
        struct B: Decodable { var boards: [JiraBoard] = [] }
        if let d = try? await client.jiraBoards(), let p = PomJSON.decode(B.self, from: d), !p.boards.isEmpty {
            boards = p.boards
            board = p.boards.first
            await loadSprint()
        }
    }

    private func loadSprint() async {
        guard let b = board else { return }
        loadingSprint = true
        struct S: Decodable { var issues: [SprintIssue] = [] }
        if let d = try? await client.jiraSprint(board: b.id), let p = PomJSON.decode(S.self, from: d) {
            sprint = p.issues
        } else { sprint = [] }
        loadingSprint = false
    }

    private func pickTicket(_ iss: SprintIssue) {
        pickedKey = iss.key
        desc = iss.summary
        refining = true
        Task {
            struct R: Decodable { var name = ""; var slug = "" }
            if let data = try? await client.suggestName(branch: iss.key, desc: iss.summary),
               let r = PomJSON.decode(R.self, from: data) {
                if !r.name.isEmpty { dispName = r.name }
                if !r.slug.isEmpty { slug = r.slug }
            }
            refining = false
        }
    }

    private func section<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            SectionLabel(text: title)
            content()
        }
    }

    private func resetCreate() {
        createError = ""; dispName = ""; slug = ""; desc = ""; picked = []; pickedKey = ""
    }

    private func toggleRepo(_ r: String) {
        if picked.contains(r) { picked.remove(r) } else { picked.insert(r) }
    }

    private func loadRepos() async {
        guard repoNames.isEmpty else { return }
        struct P: Decodable { var repos: [String] = [] }
        if let d = try? await client.repos(), let p = PomJSON.decode(P.self, from: d) {
            repoNames = p.repos
        }
    }

    private func refine() {
        let d = desc.trimmingCharacters(in: .whitespaces)
        guard !d.isEmpty else { return }
        refining = true
        Task {
            struct R: Decodable { var name = ""; var slug = "" }
            if let data = try? await client.suggestName(branch: "", desc: d),
               let r = PomJSON.decode(R.self, from: data) {
                if !r.name.isEmpty { dispName = r.name }
                if !r.slug.isEmpty { slug = r.slug }
            }
            refining = false
        }
    }

    private func create() {
        let branch = slug.trimmingCharacters(in: .whitespaces)
        guard !branch.isEmpty else { return }
        createBusy = true; createError = ""
        Task {
            do {
                let d = try await client.createWorkspace(branch: branch, repos: Array(picked), displayName: dispName)
                if let obj = try? JSONSerialization.jsonObject(with: d) as? [String: Any],
                   obj["ok"] as? Bool == false {
                    createError = (obj["error"] as? String) ?? "Create failed"
                    createBusy = false
                    return
                }
                createBusy = false
                creating = false
                await vm.load()
            } catch {
                createError = describe(error)
                createBusy = false
            }
        }
    }

    private func wideCard(_ ws: WorkspaceRow) -> some View {
        let agentState = vm.agentState(ws)
        let orbActive = agentOrbActive(agentState)
        let orbColor = agentOrbColor(agentState)
        return HStack(spacing: 12) {
            AgentOrb(color: orbColor, active: orbActive, size: 11)
            VStack(alignment: .leading, spacing: 6) {
                Text(ws.title).font(Theme.ui(15, .bold)).foregroundStyle(Theme.fg).lineLimit(1)
                HStack(spacing: 12) {
                    if let n = vm.prCount[ws.id], n > 0 {
                        footChip("\(n)", "arrow.triangle.branch", prColor(vm.prSeverity[ws.id] ?? "ok"))
                    }
                    if ws.total > 0 {
                        footChip("\(ws.running)/\(ws.total)", "square.grid.2x2", ws.running > 0 ? Theme.ok : Theme.fgMuted)
                    }
                }
            }
            Spacer(minLength: 0)
            pill("main", Theme.wsAccent)
            Image(systemName: "chevron.right").font(.system(size: 12)).foregroundStyle(Theme.dim)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Theme.wsAccent.opacity(0.35), lineWidth: 1))
    }

    private func card(_ ws: WorkspaceRow) -> some View {
        let agentState = vm.agentState(ws)
        let jira = vm.jira[ws.branch]
        let orbActive = agentOrbActive(agentState)
        let orbColor = agentOrbColor(agentState)
        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 6) {
                AgentOrb(color: orbColor, active: orbActive, size: 10).padding(.top, 2)
                Spacer(minLength: 0)
                if ws.isMain { pill("main", Theme.wsAccent) }
                else if let j = jira, !j.status.isEmpty { statusChip(j.status, jiraColor(j.category)) }
            }
            Text(ws.title)
                .font(Theme.ui(13.5, .semibold)).foregroundStyle(Theme.fg)
                .lineLimit(3).fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 0)
            HStack(spacing: 10) {
                if let n = vm.prCount[ws.id], n > 0 {
                    footChip("\(n)", "arrow.triangle.branch", prColor(vm.prSeverity[ws.id] ?? "ok"))
                }
                if ws.total > 0 {
                    footChip("\(ws.running)/\(ws.total)", "square.grid.2x2", ws.running > 0 ? Theme.ok : Theme.fgMuted)
                }
                Spacer(minLength: 0)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, minHeight: 118, alignment: .topLeading)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Theme.border, lineWidth: 1))
        .shadow(color: .black.opacity(activeThemeMode == .light ? 0.05 : 0.25), radius: 3, x: 0, y: 2)
    }

    private func footChip(_ text: String, _ icon: String, _ color: Color) -> some View {
        HStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 8))
            Text(text).font(Theme.mono(9.5))
        }
        .foregroundStyle(color)
    }

    private func prColor(_ severity: String) -> Color {
        switch severity {
        case "danger": return Theme.danger
        case "merged": return Color(hex: 0xa371f7)
        case "warn":   return Theme.warn
        default:       return Theme.ok
        }
    }

    private func statusChip(_ text: String, _ color: Color) -> some View {
        HStack(spacing: 4) {
            Circle().fill(color).frame(width: 5, height: 5)
            Text(text).font(Theme.mono(10)).foregroundStyle(color)
        }
        .padding(.horizontal, 7).padding(.vertical, 2)
        .background(color.opacity(0.12), in: Capsule())
    }

    private func pill(_ text: String, _ color: Color) -> some View {
        Text(text).font(Theme.ui(9, .medium)).foregroundStyle(color)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(color.opacity(0.16), in: Capsule())
    }

    private func jiraColor(_ category: String) -> Color {
        switch category {
        case "done": return Theme.ok
        case "indeterminate": return Theme.warn
        default: return Theme.fgMuted
        }
    }
}

struct JiraBoard: Decodable, Identifiable, Hashable {
    var id = 0, name = "", type = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decodeIfPresent(Int.self, forKey: .id) ?? 0
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        type = try c.decodeIfPresent(String.self, forKey: .type) ?? ""
    }
    enum K: String, CodingKey { case id, name, type }
}

struct SprintIssue: Decodable, Identifiable {
    var key = "", summary = "", status = "", sprint = "", assignee = "", avatar = ""
    var mine = false
    var id: String { key }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        sprint = try c.decodeIfPresent(String.self, forKey: .sprint) ?? ""
        assignee = try c.decodeIfPresent(String.self, forKey: .assignee) ?? ""
        avatar = try c.decodeIfPresent(String.self, forKey: .avatar) ?? ""
        mine = try c.decodeIfPresent(Bool.self, forKey: .mine) ?? false
    }
    enum K: String, CodingKey { case key, summary, status, sprint, assignee, avatar, mine }
}
