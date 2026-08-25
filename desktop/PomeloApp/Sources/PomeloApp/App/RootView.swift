import SwiftUI

struct RootView: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    @Environment(\.openWindow) private var openWindow
    @State private var prPeekHeight: CGFloat = 0

    var body: some View {
        _ = theme.mode
        return content
            .overlayPreferenceValue(PRPeekAnchorKey.self) { anchors in
                GeometryReader { proxy in
                    if let id = state.prPeek, let a = anchors[id] {
                        let r = proxy[a]
                        let y = min(max(8, r.minY - 6), max(8, proxy.size.height - prPeekHeight - 8))
                        ZStack(alignment: .topLeading) {
                            LeftCaret().fill(Theme.panel3)
                                .overlay(LeftCaret().stroke(Theme.border, lineWidth: 1))
                                .frame(width: 8, height: 13)
                                .offset(x: r.maxX + 3, y: r.midY - 6)
                            prPeekPanel(id)
                                .background(GeometryReader { g in
                                    Color.clear.onAppear { prPeekHeight = g.size.height }
                                        .onChange(of: g.size.height) { prPeekHeight = $0 }
                                })
                                .offset(x: r.maxX + 10, y: y)
                        }
                        .transition(.opacity.combined(with: .scale(scale: 0.96, anchor: .leading)))
                    }
                }
            }
            .animation(.easeOut(duration: 0.14), value: state.prPeek)
            .background(
                Button("") { StreamManager.shared.clearActive() }   // ⌘K clears the active terminal
                    .keyboardShortcut("k", modifiers: .command).hidden()
            )
            .overlay {
                if state.showPalette {
                    CommandPalette(show: $state.showPalette)
                        .transition(.opacity.combined(with: .scale(scale: 0.97, anchor: .top)))
                }
            }
            .animation(.spring(response: 0.25, dampingFraction: 0.85), value: state.showPalette)
            .sheet(isPresented: $state.showActivity) {
                ActivityView(scopeWsKey: state.activityScope, onClose: { state.showActivity = false })
                    .environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: $state.showPipeline) {
                PrepareMainPipelineView().environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: $state.showShared) {
                SharedServicesView(onClose: { state.showShared = false }).environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: $state.showSessionPanel) {
                SessionPanel(onClose: { state.showSessionPanel = false }).environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: $state.showSetup) {
                SetupWizard(onClose: { state.showSetup = false }).environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: $state.showCreateSession) {
                CreateSessionSheet().environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: Binding(get: { state.onboardBranch != nil }, set: { if !$0 { state.onboardBranch = nil } })) {
                OnboardSheet(branch: state.onboardBranch ?? "main", onClose: { state.onboardBranch = nil })
                    .environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: $state.showAgentSheet) {
                if let m = state.agentModel {
                    AgentSheet(model: m, title: state.agentTitle, subtitle: state.agentSubtitle,
                               runningLabel: state.agentRunningLabel,
                               onBackground: { state.backgroundAgent() },
                               onDone: { state.endAgent() },
                               onStop: { state.endAgent() })
                        .environmentObject(state).environmentObject(theme)
                }
            }
            .onChange(of: state.showSettings) {
                guard state.showSettings else { return }
                openWindow(id: "settings")
                state.showSettings = false
            }
            .onChange(of: state.openCreateWorkspace) {
                guard state.openCreateWorkspace else { return }
                openWindow(id: "create-workspace")
                state.openCreateWorkspace = false
            }
    }

    @ViewBuilder private var content: some View {
        if state.needsProject {
            WelcomeView().environmentObject(state)
        } else if let err = state.bootError {
            ContentUnavailableView("Pomelo couldn't start", systemImage: "exclamationmark.triangle", description: Text(err))
        } else {
            VStack(spacing: 0) {
                CustomHeader()
                    .zIndex(1)
                Divider().overlay(Theme.borderSoft)
                HStack(spacing: 0) {
                    if !state.sidebarCollapsed {
                        WorkspaceSidebar().frame(width: 270)
                            .zIndex(1)
                            .transition(.move(edge: .leading))
                        Divider().overlay(Theme.borderSoft)
                    }
                    Group {
                        if let err = state.configError {
                            ConfigErrorOverlay(message: err) { state.showSessionPanel = true }
                                .environmentObject(state)
                        } else if let ws = state.selectedWorkspace {
                            WorkspacePane(workspace: ws)
                        } else if state.loading {
                            VStack(spacing: 12) {
                                ProgressView().controlSize(.large)
                                Text("Starting Pomelo…").font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
                            }
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                        } else {
                            ContentUnavailableView("No workspace selected", systemImage: "square.stack.3d.up")
                        }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .allowsHitTesting(!state.showSessions)
            .background(Theme.bg)
            .background(WindowConfigurator())
            .overlayPreferenceValue(SessionAnchorKey.self) { anchor in
                if state.showSessions, let anchor {
                    GeometryReader { geo in
                        let r = geo[anchor]
                        ZStack(alignment: .topLeading) {
                            Color.black.opacity(0.001).frame(maxWidth: .infinity, maxHeight: .infinity)
                                .contentShape(Rectangle())
                                .onHover { _ in }
                                .onTapGesture { state.showSessions = false }
                            SessionsMenu()
                                .padding(.leading, r.minX).padding(.top, r.maxY + 5)
                        }
                    }
                    .zIndex(50)
                }
            }
            .overlay {
                if let m = state.rowMenu {
                    GeometryReader { geo in
                        ZStack(alignment: .topLeading) {
                            Color.black.opacity(0.001).frame(maxWidth: .infinity, maxHeight: .infinity)
                                .contentShape(Rectangle())
                                .onTapGesture { state.rowMenu = nil }
                                .overlay(RightClickCatcher { _ in state.rowMenu = nil })
                            RowContextMenu(menu: m)
                                .padding(.leading, min(m.at.x, geo.size.width - 210))
                                .padding(.top, min(m.at.y, geo.size.height - 220))
                        }
                    }
                    .zIndex(60)
                }
            }
            .sheet(item: $state.renamingWs) { ws in RenameSheet(ws: ws) }
            .confirmationDialog("Delete \(state.confirmDeleteWs?.title ?? "")?",
                                isPresented: Binding(get: { state.confirmDeleteWs != nil },
                                                     set: { if !$0 { state.confirmDeleteWs = nil } }),
                                titleVisibility: .visible) {
                Button("Delete workspace", role: .destructive) { if let ws = state.confirmDeleteWs { state.startDelete(ws) }; state.confirmDeleteWs = nil }
                Button("Cancel", role: .cancel) { state.confirmDeleteWs = nil }
            } message: {
                Text("Removes this workspace's worktrees, branch env, and databases. This can't be undone.")
            }
            .ignoresSafeArea(.container, edges: state.fullscreen ? [] : .top)
            .alert("Couldn’t switch session", isPresented: Binding(
                get: { state.switchError != nil },
                set: { if !$0 { state.switchError = nil } })) {
                Button("OK", role: .cancel) {}
            } message: { Text(state.switchError ?? "") }
            .alert("Couldn’t open session", isPresented: Binding(
                get: { state.openError != nil },
                set: { if !$0 { state.openError = nil } })) {
                Button("Choose another…") { state.openExistingSession() }
                Button("Cancel", role: .cancel) {}
            } message: { Text(state.openError ?? "") }
        }
    }
}

struct WorkspaceSidebar: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    @EnvironmentObject var ui: UIStore

    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("WORKSPACES").font(.system(size: 11, weight: .semibold)).kerning(0.8)
                    .foregroundStyle(Theme.muted)
                Spacer()
                if state.creating { ProgressView().controlSize(.mini) }
                Button { openWindow(id: "create-workspace") } label: { Image(systemName: "plus").font(.system(size: 12)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
                    .help("New workspace  ⌘N")
                    .keyboardShortcut("n", modifiers: .command)
            }
            .padding(.horizontal, 14).padding(.top, 10).padding(.bottom, 6)
            .zIndex(1)

            OpsBar()

            List {
                ForEach(state.mainWorkspaces) { ws in
                    wsRow(ws).moveDisabled(true)
                }
                ForEach(state.orderedNonMain.filter { !state.opBranches.contains($0.branch) }) { ws in
                    wsRow(ws)
                        .draggable(ws.id)
                        .dropDestination(for: String.self) { items, _ in
                            guard let dragged = items.first else { return false }
                            state.moveWorkspace(dragged, before: ws.id)
                            return true
                        }
                }
            }
            .listStyle(.plain)
            .environment(\.defaultMinListRowHeight, 1)
            .scrollContentBackground(.hidden)
        }
    }

    private func wsRow(_ ws: Workspace) -> some View {
        WsRow(ws: ws)
            .listRowInsets(EdgeInsets(top: 1, leading: 4, bottom: 1, trailing: 4))
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
    }
}

struct WsRow: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var ui: UIStore
    let ws: Workspace
    var body: some View {
        WsCard(ws: ws, selected: state.selection == ws.id, agent: state.agentStates[ws.id] ?? ws.claudeAgentState,
               prs: state.prsFor(ws.id), prsLoading: state.prsLoading, jira: state.jiraFor(ws.branch),
               onOpenPRs: { state.selection = ws.id; ui.state(for: ws.id).pane = .prs },
               onOpenJira: { state.selection = ws.id; ui.state(for: ws.id).pane = .jira },
               onPeekEnter: { state.prPeekEnter(ws.id) }, onPeekLeave: { state.prPeekLeave() })
            .contentShape(Rectangle())
            .onTapGesture { state.selection = ws.id }
            .overlay(RightClickCatcher { pt in
                state.selection = ws.id
                state.rowMenu = AppState.RowMenu(ws: ws, at: pt)
            })
    }
}

struct RowContextMenu: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var ui: UIStore
    let menu: AppState.RowMenu

    var body: some View {
        let ws = menu.ws
        VStack(spacing: 1) {
            PopItem("Rename…", icon: "pencil") { dismiss(); state.renamingWs = ws }
            PopItem("Open in editor", icon: "square.and.pencil") { dismiss(); state.openEditor(ws) }
            PopItem(ws.isMain ? "Update main from origin" : "Update from origin", icon: "arrow.triangle.2.circlepath") { dismiss(); state.pullWorkspace(ws) }
            if ws.running > 0 {
                PopItem("Stop all services", icon: "stop.fill") { dismiss(); state.stopAllServices(ws) }
            }
            if !ws.isMain {
                PopItem("Pull requests", icon: "arrow.triangle.pull") { dismiss(); state.selection = ws.id; ui.state(for: ws.id).pane = .prs }
                PopItem("Jira ticket", icon: "ticket") { dismiss(); state.selection = ws.id; ui.state(for: ws.id).pane = .jira }
                Divider().padding(.vertical, 2)
                PopItem("Delete workspace", icon: "trash", destructive: true) { dismiss(); state.confirmDeleteWs = ws }
            }
        }
        .padding(5).frame(width: 200)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border))
        .shadow(color: .black.opacity(0.35), radius: 14, y: 5)
    }
    private func dismiss() { state.rowMenu = nil }
}

func humanizeBranch(_ b: String) -> String {
    let parts = b.split(separator: "-").map(String.init)
    guard !parts.isEmpty else { return b }
    var i = 0, prefix = ""
    if parts.count >= 2, parts[0].allSatisfy(\.isLetter), parts[1].allSatisfy(\.isNumber) {
        prefix = parts[0].uppercased() + "-" + parts[1]; i = 2
    }
    let rest = parts[i...].joined(separator: " ")
    let restCap = rest.isEmpty ? "" : rest.prefix(1).uppercased() + rest.dropFirst()
    if prefix.isEmpty { return restCap.isEmpty ? b : restCap }
    return restCap.isEmpty ? prefix : "\(prefix) \(restCap)"
}

struct RenameSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.dismiss) private var dismiss
    let ws: Workspace
    @State private var name = ""
    @State private var suggesting = false
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "pencil.line").font(.system(size: 13)).foregroundStyle(Theme.accent)
                Text("Rename workspace").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            }
            Text("Display label only — the git branch stays the same.")
                .font(.system(size: 11.5)).foregroundStyle(Theme.dim).padding(.top, 4)

            HStack(spacing: 6) {
                Image(systemName: "arrow.triangle.branch").font(.system(size: 10)).foregroundStyle(Theme.dim)
                Text(ws.branch).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted).lineLimit(1).truncationMode(.middle)
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.bg, in: RoundedRectangle(cornerRadius: 7))
            .padding(.top, 14)

            ZStack(alignment: .leading) {
                TextField("Display name", text: $name)
                    .textFieldStyle(.plain).font(.system(size: 14))
                    .focused($focused)
                    .disabled(suggesting)
                    .opacity(suggesting ? 0 : 1)
                if suggesting {
                    Text("Refining with Claude…").font(.system(size: 14)).foregroundStyle(Theme.fgMuted)
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .background(Theme.bg, in: RoundedRectangle(cornerRadius: 9))
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(focused ? Theme.accent : Theme.border, lineWidth: focused ? 1.5 : 1))
            .rainbowShimmer(active: suggesting)
            .onSubmit(save)
            .padding(.top, 10)

            HStack(spacing: 8) {
                Button(action: refine) {
                    Label(suggesting ? "Refining…" : "Refine with Claude", systemImage: "sparkles").font(.system(size: 11.5))
                }
                .buttonStyle(.plain).foregroundStyle(suggesting ? Theme.dim : Theme.accent).disabled(suggesting)
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
                    .keyboardShortcut(.cancelAction)
                Button("Save", action: save).buttonStyle(.borderedProminent).tint(Theme.accent)
                    .keyboardShortcut(.defaultAction)
            }
            .padding(.top, 16)
        }
        .padding(20).frame(width: 440)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Theme.borderSoft))
        .onAppear {
            name = ws.displayName ?? humanizeBranch(ws.branch)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { focused = true }
        }
    }

    private func refine() {
        suggesting = true
        Task {
            let n = await state.suggestName(branch: ws.branch)
            withAnimation(.easeOut(duration: 0.2)) { name = n; suggesting = false }
        }
    }

    private func save() {
        state.renameWorkspace(ws, to: name.trimmingCharacters(in: .whitespaces))
        dismiss()
    }
}

func slugify(_ s: String) -> String {
    let lowered = s.lowercased()
    var out = ""; var prevDash = false
    for ch in lowered {
        if ch.isLetter || ch.isNumber { out.append(ch); prevDash = false }
        else if !prevDash { out.append("-"); prevDash = true }
    }
    return out.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
}

struct CreateWorkspaceView: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    @Environment(\.dismiss) private var dismiss
    @State private var ticket = ""
    @State private var desc = ""
    @State private var autoDesc = ""   // last summary auto-filled from a ticket; empty = user typed
    @State private var picked: Set<String> = []
    @State private var busy = false
    @State private var suggesting = false
    @State private var name = ""          // display label (editable, keeps case)
    @State private var slugOverride = ""  // Claude-refined slug; empty = use autoSlug
    @State private var boards: [JiraBoard] = []
    @State private var board: JiraBoard?
    @State private var sprint: [SprintIssue] = []
    @State private var loadingSprint = false
    @State private var showSuggest = false
    @State private var preview: JiraDetail?
    @State private var previewLoading = false
    @State private var refined = false
    @FocusState private var ticketFocused: Bool

    private var autoSlug: String { [slugify(ticket), slugify(desc)].filter { !$0.isEmpty }.joined(separator: "-") }
    private var slug: String { slugOverride.isEmpty ? autoSlug : slugOverride }

    private var suggestions: [SprintIssue] {
        let existing = Set(state.workspaces.compactMap { jiraKey($0.branch) })
        let q = ticket.trimmingCharacters(in: .whitespaces)
        let avail = sprint.filter { !existing.contains($0.key.uppercased()) && (!state.jiraOnlyMine || $0.mine) }
        let scored: [(iss: SprintIssue, score: Int)] = avail.compactMap { iss in
            if q.isEmpty { return (iss, 0) }
            let s = [Fuzzy.score(q, iss.key), Fuzzy.score(q, iss.summary)].compactMap { $0 }.max()
            return s.map { (iss, $0) }
        }
        return scored.sorted {
            $0.iss.mine != $1.iss.mine ? ($0.iss.mine && !$1.iss.mine)
                : ($0.score != $1.score ? $0.score > $1.score : $0.iss.key < $1.iss.key)
        }.map(\.iss)
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                RoundedRectangle(cornerRadius: 6)
                    .fill(LinearGradient(colors: [.orange, .pink], startPoint: .top, endPoint: .bottom))
                    .frame(width: 20, height: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Create workspace").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text("Pick a sprint ticket or describe the work").font(.system(size: 11)).foregroundStyle(Theme.dim)
                }
                Spacer()
            }
            .padding(.horizontal, 20).padding(.vertical, 14)
            Divider().overlay(Theme.borderSoft)

            HStack(spacing: 0) {
                inputColumn.frame(width: 360)
                Divider().overlay(Theme.borderSoft)
                previewColumn.frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .frame(maxHeight: .infinity)

            Divider().overlay(Theme.borderSoft)
            HStack(spacing: 10) {
                if !refined && !slug.isEmpty {
                    Text("Refine with Claude before creating").font(.system(size: 11)).foregroundStyle(Theme.dim)
                }
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted).disabled(busy)
                    .keyboardShortcut(.cancelAction)
                Button { create() } label: { Text(busy ? "Creating…" : "Create") }
                    .buttonStyle(.borderedProminent).tint(Theme.accent)
                    .disabled(slug.isEmpty || busy || !refined)
                    .rainbowShimmer(active: busy, cornerRadius: 6)
            }
            .padding(.horizontal, 20).padding(.vertical, 12)
        }
        .frame(minWidth: 940, minHeight: 620)
        .background(Theme.bgSoft)
        .task { await loadBoards(); theme.applyToWindow() }
        .onChange(of: board) { Task { await loadSprint() } }
        .onChange(of: ticketFocused) { _, f in if f { showSuggest = true } }
        .onChange(of: ticket) { showSuggest = true; schedulePreview(); refined = false; name = ""; slugOverride = "" }
        .onChange(of: desc) { refined = false }
    }

    private var inputColumn: some View {
        VStack(alignment: .leading, spacing: 16) {
            jiraSection.zIndex(1)
            field("Description", hint: "optional — used alone if no ticket") {
                inputField("add login", $desc)
            }
            field("Repos", hint: "empty = default combo") {
                ScrollView {
                    VStack(alignment: .leading, spacing: 1) {
                        ForEach(state.allRepoNames, id: \.self) { name in repoRow(name) }
                    }.padding(4)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                .background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
            }
            Spacer(minLength: 0)
        }
        .padding(18)
    }

    private var previewColumn: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let p = preview {
                HStack(spacing: 8) {
                    Text(p.key).font(Theme.mono(12, .medium)).foregroundStyle(Theme.accent)
                    if !p.status.isEmpty { metaBadge(p.status, Theme.fgMuted) }
                    Spacer()
                    Button { if let u = URL(string: p.url) { NSWorkspace.shared.open(u) } } label: {
                        Image(systemName: "arrow.up.forward.square").font(.system(size: 12))
                    }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted).help("Open in Jira")
                }
                Text(p.summary).font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                    .padding(.top, 6).fixedSize(horizontal: false, vertical: true)
                Divider().overlay(Theme.borderSoft).padding(.vertical, 10)
                ScrollView { MarkdownText(p.description).frame(maxWidth: .infinity, alignment: .leading) }
                    .frame(maxHeight: .infinity)
            } else {
                VStack(spacing: 8) {
                    Image(systemName: previewLoading ? "ellipsis" : "doc.text.magnifyingglass")
                        .font(.system(size: 30)).foregroundStyle(Theme.dim)
                    Text(previewLoading ? "Loading ticket…" : "Pick a sprint ticket to preview it")
                        .font(.system(size: 12.5)).foregroundStyle(Theme.fgMuted)
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }

            Divider().overlay(Theme.borderSoft).padding(.vertical, 10)
            nameSlugSection
        }
        .padding(18)
    }

    private var nameSlugSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            field("Name", hint: "human-friendly label — kept as-is") {
                ZStack(alignment: .leading) {
                    TextField("Display name", text: $name).textFieldStyle(.plain).font(.system(size: 13))
                        .disabled(suggesting).opacity(suggesting ? 0 : 1)
                    if suggesting { Text("Refining with Claude…").font(.system(size: 13)).foregroundStyle(Theme.fgMuted) }
                }
                .padding(.horizontal, 10).padding(.vertical, 8)
                .background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border))
                .rainbowShimmer(active: suggesting, cornerRadius: 8)
            }
            field("Slug", hint: "git branch · read-only — refined by Claude") {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.triangle.branch").font(.system(size: 10)).foregroundStyle(Theme.dim)
                    Text(slug.isEmpty ? "—" : slug).font(Theme.mono(12)).foregroundStyle(slug.isEmpty ? Theme.dim : Theme.accent)
                        .lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 10).padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.bg.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderSoft))
                .rainbowShimmer(active: suggesting, cornerRadius: 8)
            }
            Button(action: refine) {
                Label(suggesting ? "Refining…" : "Refine name & slug with Claude", systemImage: "sparkles").font(.system(size: 11.5))
            }
            .buttonStyle(.plain).foregroundStyle(suggesting || autoSlug.isEmpty ? Theme.dim : Theme.accent)
            .disabled(suggesting || autoSlug.isEmpty)
        }
    }

    @State private var previewTask: Task<Void, Never>?
    private func schedulePreview() {
        previewTask?.cancel()
        guard let key = jiraKey(ticket) else { preview = nil; return }
        previewTask = Task {
            try? await Task.sleep(nanoseconds: 350_000_000)
            if Task.isCancelled { return }
            previewLoading = true
            let d = await state.jiraIssue(key)
            if Task.isCancelled { return }
            preview = d; previewLoading = false
        }
    }

    private var jiraSection: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Text("Jira ticket").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.fgSoft)
                Spacer()
                if !boards.isEmpty {
                    ChipSelect(text: board?.name ?? "board", color: Theme.accent,
                               options: boards.map(\.name), current: board?.name) { pick in
                        board = boards.first { $0.name == pick }
                    }
                }
            }
            inputField("PROJ-800", $ticket).focused($ticketFocused)
                .overlay(alignment: .topLeading) {
                    if showSuggest && !suggestions.isEmpty {
                        suggestionsList.offset(y: 38).zIndex(10)
                    }
                }
            Text(loadingSprint ? "loading sprint…" : "optional — pick from the sprint or type a key")
                .font(.system(size: 10.5)).foregroundStyle(Theme.dim)
        }
    }

    private var suggestionsList: some View {
        let h = min(CGFloat(min(suggestions.count, 20)) * 46 + 8, 300)
        return ScrollView {
            VStack(alignment: .leading, spacing: 1) {
                ForEach(suggestions.prefix(20)) { iss in
                    Button { pick(iss) } label: { suggestionRow(iss) }.buttonStyle(.plain)
                }
            }.padding(4)
        }
        .frame(height: h)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border))
        .shadow(color: .black.opacity(0.35), radius: 12, y: 6)
    }

    private func suggestionRow(_ iss: SprintIssue) -> some View {
        HStack(alignment: .top, spacing: 9) {
            avatar(iss)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(iss.key).font(Theme.mono(11, .medium)).foregroundStyle(Theme.accent)
                    Text(iss.summary).font(.system(size: 11.5)).foregroundStyle(Theme.fg).lineLimit(1)
                }
                HStack(spacing: 6) {
                    Text(iss.mine ? "You" : (iss.assignee.isEmpty ? "Unassigned" : iss.assignee))
                        .font(.system(size: 10)).foregroundStyle(iss.mine ? Theme.accent : Theme.fgMuted)
                    if !iss.sprint.isEmpty {
                        metaBadge(iss.sprint, Theme.dim)
                    }
                    if !iss.status.isEmpty { metaBadge(iss.status, Theme.dim) }
                }
            }
            Spacer(minLength: 6)
            if iss.mine {
                Text("mine").font(.system(size: 8.5, weight: .bold)).foregroundStyle(Theme.accent)
                    .padding(.horizontal, 4).padding(.vertical, 1).background(Theme.accentSoft, in: Capsule())
            }
        }
        .padding(.horizontal, 8).padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }

    private func avatar(_ iss: SprintIssue) -> some View {
        let initial = String((iss.mine ? "You" : iss.assignee).prefix(1)).uppercased()
        return Circle()
            .fill(iss.mine ? Theme.accent.opacity(0.25) : Theme.hover)
            .frame(width: 20, height: 20)
            .overlay(Text(initial.isEmpty ? "?" : initial).font(.system(size: 9, weight: .semibold)).foregroundStyle(iss.mine ? Theme.accent : Theme.fgMuted))
            .padding(.top, 1)
    }

    private func metaBadge(_ text: String, _ c: Color) -> some View {
        Text(text).font(.system(size: 9)).foregroundStyle(c)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background(Theme.chip, in: Capsule())
            .lineLimit(1)
    }

    private func pick(_ iss: SprintIssue) {
        ticket = iss.key
        // Refresh desc from the picked ticket unless the user typed their own —
        // else switching tickets keeps the old summary and the refined name/slug
        // describes the wrong ticket.
        if desc.trimmingCharacters(in: .whitespaces).isEmpty || desc == autoDesc {
            desc = iss.summary
            autoDesc = iss.summary
        }
        showSuggest = false
        ticketFocused = false
    }

    private func loadBoards() async {
        let bs = await state.jiraBoards()
        boards = bs
        if board == nil {
            let last = UserDefaults.standard.integer(forKey: "jiraBoard")
            board = bs.first { $0.id == last } ?? bs.first
        }
        await loadSprint()
    }

    private func loadSprint() async {
        guard let b = board else { return }
        UserDefaults.standard.set(b.id, forKey: "jiraBoard")
        loadingSprint = true
        sprint = await state.jiraSprint(board: b.id)
        loadingSprint = false
    }

    private func refine() {
        suggesting = true
        Task {
            let r = await state.suggestNameSlug(seed: autoSlug, desc: desc)
            withAnimation(.easeOut(duration: 0.2)) { name = r.name; slugOverride = r.slug; suggesting = false; refined = true }
        }
    }

    @ViewBuilder private func field(_ label: String, hint: String, @ViewBuilder _ content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label).font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.fgSoft)
            content()
            Text(hint).font(.system(size: 10.5)).foregroundStyle(Theme.dim)
        }
    }

    private func inputField(_ placeholder: String, _ text: Binding<String>) -> some View {
        TextField(placeholder, text: text)
            .textFieldStyle(.plain)
            .font(.system(size: 13))
            .padding(.horizontal, 10).padding(.vertical, 8)
            .background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border))
    }

    private func repoRow(_ name: String) -> some View {
        Button {
            if picked.contains(name) { picked.remove(name) } else { picked.insert(name) }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: picked.contains(name) ? "checkmark.square.fill" : "square")
                    .font(.system(size: 13)).foregroundStyle(picked.contains(name) ? Theme.accent : Theme.dim)
                Text(name).font(.system(size: 12.5)).foregroundStyle(Theme.fg)
                Spacer()
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func create() {
        state.startCreate(branch: slug, repos: Array(picked), displayName: name)
        dismiss()
    }
}

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
        case "awaiting_input": return Theme.danger
        default:               return Theme.dim
        }
    }
    private var orbActive: Bool { agent == "thinking" || agent == "tool_use" || agent == "awaiting_input" }

    private var openPRs: [WorkspacePR] { prs.filter { $0.pr != nil } }
    private var prColor: Color {
        var anyMerged = false, worst = 0
        for p in openPRs {
            guard let pr = p.pr else { continue }
            if pr.state == "MERGED" { anyMerged = true; continue }
            if pr.conflict || pr.checks == .fail || pr.review == .changes { worst = max(worst, 2) }
            else if pr.checks == .pending || pr.review == .review { worst = max(worst, 1) }
        }
        if worst == 2 { return Theme.danger }
        if anyMerged { return Color(hex: 0xa371f7) }
        return worst == 1 ? Theme.warn : Theme.ok
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
        .anchorPreference(key: PRPeekAnchorKey.self, value: .bounds) { [ws.id: $0] }
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
