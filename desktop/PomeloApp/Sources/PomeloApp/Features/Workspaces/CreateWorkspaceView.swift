import SwiftUI

private struct SuggestionRowView: View {
    let iss: SprintIssue
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 9) {
                avatar
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(iss.key).font(Theme.mono(11, .medium)).foregroundStyle(Theme.accent)
                        Text(iss.summary).font(.system(size: 11.5)).foregroundStyle(Theme.fg).lineLimit(1)
                    }
                    HStack(spacing: 6) {
                        Text(iss.mine ? "You" : (iss.assignee.isEmpty ? "Unassigned" : iss.assignee))
                            .font(.system(size: 10)).foregroundStyle(iss.mine ? Theme.accent : Theme.fgMuted)
                        if !iss.sprint.isEmpty { badge(iss.sprint) }
                        if !iss.status.isEmpty { badge(iss.status) }
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
            .background(hover ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }

    private var avatar: some View {
        let initial = String((iss.mine ? "You" : iss.assignee).prefix(1)).uppercased()
        return Circle()
            .fill(iss.mine ? Theme.accent.opacity(0.25) : Theme.hover)
            .frame(width: 20, height: 20)
            .overlay(Text(initial.isEmpty ? "?" : initial).font(.system(size: 9, weight: .semibold)).foregroundStyle(iss.mine ? Theme.accent : Theme.fgMuted))
            .padding(.top, 1)
    }

    private func badge(_ text: String) -> some View {
        Text(text).font(.system(size: 9)).foregroundStyle(Theme.dim)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background(Theme.chip, in: Capsule()).lineLimit(1)
    }
}

struct CreateWorkspaceView: View {
    @Environment(AppState.self) var state
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
    @State private var justPicked = false
    @State private var preview: JiraDetail?
    @State private var previewLoading = false
    @State private var refined = false
    @State private var useJira = false
    @FocusState private var ticketFocused: Bool

    private var autoSlug: String { slugify(jiraOn ? ticket : name) }
    private var slug: String { slugOverride.isEmpty ? autoSlug : slugOverride }

    private var suggestions: [SprintIssue] {
        let existing = Set(state.workspaces.compactMap { jiraKey($0.branch) })
        return SprintSuggest.rank(sprint, existing: existing, query: ticket, onlyMine: state.jiraOnlyMine)
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                Image(nsImage: NSApplication.shared.applicationIconImage)
                    .resizable().frame(width: 22, height: 22)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Create workspace").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text(jiraOn ? "Pick a sprint ticket or describe the work" : "Describe the work")
                        .font(.system(size: 11)).foregroundStyle(Theme.dim)
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
                    .disabled(slug.isEmpty || busy)
                    .rainbowShimmer(active: busy, cornerRadius: 6)
            }
            .padding(.horizontal, 20).padding(.vertical, 12)
        }
        .frame(minWidth: 940, minHeight: 620)
        .background(Theme.bgSoft)
        .task { await loadBoards(); theme.applyToWindow() }
        .onChange(of: board) { Task { await loadSprint() } }
        .onChange(of: ticketFocused) { _, f in showSuggest = f && !justPicked }
        .onChange(of: ticket) { if !justPicked { showSuggest = true }; schedulePreview(); refined = false; name = ""; slugOverride = "" }
    }

    private var jiraOn: Bool { state.jiraConfigured && useJira }

    private var trackerPicker: some View {
        HStack(spacing: 6) {
            trackerChip("No ticket", on: !useJira) { useJira = false }
            trackerChip("Jira", on: useJira) { useJira = true }
            Spacer()
        }
    }

    private func trackerChip(_ label: String, on: Bool, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            Text(label).font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(on ? Theme.accent : Theme.fgMuted)
                .padding(.horizontal, 10).padding(.vertical, 5)
                .background(on ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 7))
                .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(on ? Theme.accent.opacity(0.5) : Theme.borderSoft))
        }.buttonStyle(.plain)
    }

    private var inputColumn: some View {
        VStack(alignment: .leading, spacing: 16) {
            if state.jiraConfigured { trackerPicker }
            if jiraOn { jiraSection.zIndex(1) }
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
            if !jiraOn {
                nameSlugSection
                Spacer(minLength: 0)
            } else {
                jiraPreviewColumn
            }
        }
        .padding(18)
    }

    private var jiraPreviewColumn: some View {
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
            field("Slug", hint: "git branch · edit or refine with Claude") {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.triangle.branch").font(.system(size: 10)).foregroundStyle(Theme.dim)
                    TextField("—", text: Binding(
                        get: { slug },
                        set: { slugOverride = slugify($0); refined = true }
                    ))
                    .textFieldStyle(.plain).font(Theme.mono(12)).foregroundStyle(Theme.accent)
                    .disabled(suggesting)
                }
                .padding(.horizontal, 10).padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border))
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
                .opacity(showSuggest && !suggestions.isEmpty ? 0 : 1)
        }
    }

    private var suggestionsList: some View {
        let h = min(CGFloat(min(suggestions.count, 20)) * 46 + 8, 300)
        return ScrollView {
            VStack(alignment: .leading, spacing: 1) {
                ForEach(suggestions.prefix(20)) { iss in
                    SuggestionRowView(iss: iss) { pick(iss) }
                }
            }.padding(4)
        }
        .frame(height: h)
        .background(RoundedRectangle(cornerRadius: 8).fill(Theme.bgSoft))
        .background(RoundedRectangle(cornerRadius: 8).fill(Theme.panel3))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border))
        .compositingGroup()
        .shadow(color: .black.opacity(0.45), radius: 14, y: 7)
    }

    private func metaBadge(_ text: String, _ c: Color) -> some View {
        Text(text).font(.system(size: 9)).foregroundStyle(c)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background(Theme.chip, in: Capsule())
            .lineLimit(1)
    }

    private func pick(_ iss: SprintIssue) {
        // @FocusState updates async, so guard the re-show with a plain flag that
        // outlives the pick's onChange(ticket)/refocus before clearing itself.
        justPicked = true
        Task { try? await Task.sleep(nanoseconds: 400_000_000); justPicked = false }
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
            let r = await state.suggestNameSlug(seed: autoSlug, desc: jiraOn ? desc : name)
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
