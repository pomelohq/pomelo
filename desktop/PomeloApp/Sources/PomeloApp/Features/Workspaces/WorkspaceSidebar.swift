import SwiftUI

struct WorkspaceSidebar: View {
    @Environment(AppState.self) var state
    @EnvironmentObject var theme: ThemeManager
    @Environment(UIStore.self) var ui

    @Environment(\.openWindow) private var openWindow
    @State private var dragId: String?
    @State private var dragTranslation: CGFloat = 0
    @State private var heightStore = HeightStore()
    private var heights: [String: CGFloat] { heightStore.h }

    private let rowSpacing: CGFloat = 2
    private var items: [Workspace] { state.orderedNonMain.filter { !state.opBranches.contains($0.branch) } }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("WORKSPACES").font(.system(size: 11, weight: .semibold)).kerning(0.8)
                    .foregroundStyle(Theme.muted)
                Spacer()
                if state.creating { Spinner(size: 11) }
                IconButton("plus", size: 12, tip: "New workspace  ⌘N") { openWindow(id: "create-workspace") }
                    .keyboardShortcut("n", modifiers: .command)
            }
            .padding(.horizontal, 14).padding(.top, 10).padding(.bottom, 6)
            .zIndex(1)

            OpsBar()

            ScrollView {
                // Not Lazy: LazyVStack's height estimate drifts on fast flicks (blank gaps).
                VStack(spacing: rowSpacing) {
                    ForEach(state.mainWorkspaces) { ws in WsRow(ws: ws) }
                    ForEach(Array(items.enumerated()), id: \.element.id) { idx, ws in
                        WsRow(ws: ws)
                            // Measure only while dragging — a GeometryReader on every row
                            // re-measures on every layout pass and janks idle scrolling.
                            .background { if dragId != nil { HeightReader(id: ws.id) } }
                            .offset(y: rowOffset(idx))
                            .scaleEffect(dragId == ws.id ? 1.03 : 1)
                            .shadow(color: dragId == ws.id ? .black.opacity(0.28) : .clear,
                                    radius: dragId == ws.id ? 9 : 0, y: 5)
                            .opacity(dragId != nil && dragId != ws.id ? 0.85 : 1)
                            .zIndex(dragId == ws.id ? 2 : 0)
                            .animation(dragId == ws.id ? nil : .spring(response: 0.26, dampingFraction: 0.82), value: rowOffset(idx))
                            .animation(.spring(response: 0.24, dampingFraction: 0.8), value: dragId)
                            .gesture(reorderGesture(idx: idx, ws: ws))
                    }
                }
                .padding(.horizontal, 4).padding(.vertical, 4)
                .onPreferenceChange(RowHeightKey.self) { heightStore.h = $0 }
            }
            .scrollContentBackground(.hidden)
        }
        .perfTag("Sidebar")
    }

    private func HeightReader(id: String) -> some View {
        GeometryReader { g in Color.clear.preference(key: RowHeightKey.self, value: [id: g.size.height]) }
    }

    private var midYs: [CGFloat] {
        var y: CGFloat = 0
        return items.map { ws in
            let h = heights[ws.id] ?? 44
            defer { y += h + rowSpacing }
            return y + h / 2
        }
    }

    private var targetIndex: Int {
        guard let dragId, let from = items.firstIndex(where: { $0.id == dragId }) else { return 0 }
        let mids = midYs
        guard from < mids.count else { return from }
        let mid = mids[from] + dragTranslation
        var t = from
        while t > 0 && mid < mids[t - 1] { t -= 1 }
        while t < items.count - 1 && mid > mids[t + 1] { t += 1 }
        return t
    }

    private func rowOffset(_ idx: Int) -> CGFloat {
        guard let dragId, let from = items.firstIndex(where: { $0.id == dragId }) else { return 0 }
        if idx == from { return dragTranslation }
        let dh = (heights[dragId] ?? 44) + rowSpacing
        let to = targetIndex
        if from < to, idx > from, idx <= to { return -dh }
        if from > to, idx >= to, idx < from { return dh }
        return 0
    }

    private func reorderGesture(idx: Int, ws: Workspace) -> some Gesture {
        DragGesture(minimumDistance: 6, coordinateSpace: .global)
            .onChanged { v in
                if dragId == nil { dragId = ws.id }
                dragTranslation = v.translation.height
            }
            .onEnded { _ in
                let t = targetIndex
                state.moveWorkspace(ws.id, toIndex: t)
                dragId = nil
                dragTranslation = 0
            }
    }
}

final class HeightStore { var h: [String: CGFloat] = [:] }

struct RowHeightKey: PreferenceKey {
    static let defaultValue: [String: CGFloat] = [:]
    static func reduce(value: inout [String: CGFloat], nextValue: () -> [String: CGFloat]) {
        value.merge(nextValue()) { _, n in n }
    }
}

// Keeps the selected + recently-visited workspace panes mounted (LRU-capped) and
// just toggles visibility, so switching between them is instant — no remount, no
// claude reconnect / "starting…" flash. Panes beyond the cap are dropped (reopen
// pays a one-time load).
struct KeepAliveWorkspaceHost: View {
    @Environment(AppState.self) var state
    @State private var mounted: [String] = []
    private let cap = 4

    var body: some View {
        ZStack {
            ForEach(mounted, id: \.self) { id in
                if let ws = state.workspaces.first(where: { $0.id == id }) {
                    WorkspacePane(workspace: ws)
                        .opacity(id == state.selection ? 1 : 0)
                        .allowsHitTesting(id == state.selection)
                        .zIndex(id == state.selection ? 1 : 0)
                }
            }
        }
        .onAppear(perform: sync)
        .onChange(of: state.selection) { sync() }
    }

    private func sync() {
        guard let sel = state.selection else { return }
        var m = mounted.filter { $0 != sel }
        m.insert(sel, at: 0)
        if m.count > cap { m = Array(m.prefix(cap)) }
        mounted = m.filter { id in state.workspaces.contains { $0.id == id } }
    }
}

struct WsRow: View {
    @Environment(AppState.self) var state
    @Environment(UIStore.self) var ui
    @EnvironmentObject var theme: ThemeManager
    let ws: Workspace
    var body: some View {
        WsCard(ws: ws, selected: state.selection == ws.id, agent: state.agentStates[ws.id] ?? ws.claudeAgentState,
               prs: state.prsFor(ws.id), severity: state.prSeverityFor(ws.id), prsLoading: state.prsLoading,
               pullOn: ws.isMain && state.syncOn, pullIntervalSec: state.syncIntervalSec, pulling: ws.isMain && state.syncPulling,
               pulledAt: ws.isMain ? state.syncPulledAt : nil, pullProgress: ws.isMain ? state.syncProgress : [],
               jira: state.jiraFor(ws.branch), themeMode: theme.mode,
               onOpenPRs: { state.selection = ws.id; ui.state(for: ws.id).pane = .prs },
               onOpenJira: { state.selection = ws.id; ui.state(for: ws.id).pane = .jira },
               onPeekEnter: { state.prPeekEnter(ws.id) }, onPeekLeave: { state.prPeekLeave() })
            .equatable()
            .contentShape(Rectangle())
            .onTapGesture { state.selection = ws.id }
            .overlay(RightClickCatcher { pt in
                state.selection = ws.id
                state.rowMenu = AppState.RowMenu(ws: ws, at: pt)
            })
            .perfTag("WsRow")
    }
}

struct RowContextMenu: View {
    @Environment(AppState.self) var state
    @Environment(UIStore.self) var ui
    let menu: AppState.RowMenu

    var body: some View {
        let ws = menu.ws
        VStack(spacing: 1) {
            PopItem("Rename…", icon: "pencil") { dismiss(); state.renamingWs = ws }
            PopItem("Add repo…", icon: "plus.rectangle.on.folder") { dismiss(); state.addRepoWs = ws }
            PopItem("Open in editor", icon: "square.and.pencil") { dismiss(); state.openEditor(ws) }
            if ws.isMain {
                PopItem("Update main from origin", icon: "arrow.triangle.2.circlepath") { dismiss(); state.updateMainWs = ws }
            }
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

struct AddRepoSheet: View {
    @Environment(AppState.self) var state
    @Environment(\.dismiss) private var dismiss
    let ws: Workspace
    @State private var picked: Set<String> = []

    private var available: [String] {
        let have = Set(ws.repos.map(\.name))
        return state.allRepoNames.filter { !have.contains($0) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "plus.rectangle.on.folder").font(.system(size: 13)).foregroundStyle(Theme.accent)
                Text("Add repo").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            }
            Text("Fork the selected repos onto this workspace's branch and wire up their env, ports, and services.")
                .font(.system(size: 11.5)).foregroundStyle(Theme.dim).padding(.top, 4)

            HStack(spacing: 6) {
                Image(systemName: "arrow.triangle.branch").font(.system(size: 10)).foregroundStyle(Theme.dim)
                Text(ws.branch).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted).lineLimit(1).truncationMode(.middle)
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.bg, in: RoundedRectangle(cornerRadius: 7))
            .padding(.top, 14)

            if available.isEmpty {
                Text("Every configured repo is already in this workspace.")
                    .font(.system(size: 12.5)).foregroundStyle(Theme.fgMuted)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 18)
            } else {
                ScrollView {
                    VStack(spacing: 1) { ForEach(available, id: \.self) { name in repoRow(name) } }
                }
                .frame(maxHeight: 220).padding(.top, 10)
            }

            HStack(spacing: 8) {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
                    .keyboardShortcut(.cancelAction)
                Button("Add") { state.startAddRepo(ws, repos: Array(picked)); dismiss() }
                    .buttonStyle(.borderedProminent).tint(Theme.accent)
                    .keyboardShortcut(.defaultAction).disabled(picked.isEmpty)
            }
            .padding(.top, 16)
        }
        .padding(20).frame(width: 420)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Theme.borderSoft))
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
            .padding(.horizontal, 8).padding(.vertical, 6).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

struct RenameSheet: View {
    @Environment(AppState.self) var state
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

