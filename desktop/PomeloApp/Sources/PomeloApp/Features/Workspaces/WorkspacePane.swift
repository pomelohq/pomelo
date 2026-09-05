import SwiftUI

enum PaneKind: String, CaseIterable, Identifiable {
    case claude = "Claude", services = "Services", git = "Git", jira = "Jira", database = "Database", review = "Review", files = "Files"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .claude:   return "sparkles"
        case .services: return "square.grid.2x2"
        case .git:      return "arrow.triangle.branch"
        case .jira:     return "ticket"
        case .database: return "cylinder.split.1x2"
        case .review:   return "doc.text.magnifyingglass"
        case .files:    return "folder"
        }
    }
}

struct TermTab: Identifiable, Equatable {
    let id = UUID()
    var title: String
    let holder: String
    var autorun: String? = nil
}

@MainActor
@Observable final class PaneState {
    var pane: PaneKind = .services { didSet { persist() } }
    var terms: [TermTab] = []
    var selTerm: UUID?
    var drawerOpen = false
    var drawerHeight: CGFloat = 300
    var agentOpen = false { didSet { persist() } }
    var funcVisible = true { didSet { persist() } }
    var agentWidth: Double = 480 { didSet { persist() } }

    private var loaded = false

    private func persist() {
        guard loaded, !wsKey.isEmpty else { return }
        UserDefaults.standard.set(["pane": pane.rawValue, "agent": agentOpen, "func": funcVisible, "w": agentWidth],
                                  forKey: "paneState.\(wsKey)")
    }

    func loadPersisted() {
        defer { loaded = true }
        guard let d = UserDefaults.standard.dictionary(forKey: "paneState.\(wsKey)") else { return }
        if let p = d["pane"] as? String, let k = PaneKind(rawValue: p) { pane = k }
        if let a = d["agent"] as? Bool { agentOpen = a }
        if let f = d["func"] as? Bool { funcVisible = f }
        if let w = d["w"] as? Double { agentWidth = w }
    }

    func toggleAgent() {
        agentOpen.toggle()
        ensureOneVisible()
    }

    func selectFunc(_ kind: PaneKind) {
        if funcVisible && pane == kind { funcVisible = false }
        else { pane = kind; funcVisible = true }
        ensureOneVisible()
    }

    private func ensureOneVisible() {
        if !funcVisible && !agentOpen { funcVisible = true }
    }
    var wsKey = ""
    private var termSeq = 0

    private var safeWs: String {
        wsKey.replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: ":", with: "-")
    }

    func makeShellTab(autorun: String? = nil, title: String? = nil) -> TermTab {
        termSeq += 1
        return TermTab(title: title ?? "zsh \(termSeq)", holder: "appsh-\(safeWs)-\(termSeq)", autorun: autorun)
    }

    func newTerminal() {
        let t = makeShellTab()
        terms.append(t); selTerm = t.id; drawerOpen = true
    }

    func closeSelected() {
        guard let id = selTerm, let t = terms.first(where: { $0.id == id }) else { return }
        let killable = t.holder.hasPrefix("appsh-") || t.holder.hasPrefix("sh-") || t.holder.hasPrefix("reposh-")
        if killable {
            let h = t.holder
            PaneStore.kill(paneID: "pty:" + h)
        }
        terms.removeAll { $0.id == id }
        selTerm = terms.last?.id
        if terms.isEmpty { drawerOpen = false }
    }

    func toggleDrawer() {
        if drawerOpen { drawerOpen = false; return }
        if terms.isEmpty {
            let t = makeShellTab()
            terms.append(t); selTerm = t.id
        }
        drawerOpen = true
    }
}

@MainActor
@Observable final class UIStore {
    private var map: [String: PaneState] = [:]
    func state(for id: String) -> PaneState {
        if let s = map[id] { return s }
        let s = PaneState(); s.wsKey = id; s.loadPersisted(); map[id] = s; return s
    }
}

struct WorkspacePane: View {
    let workspace: Workspace
    @Environment(UIStore.self) var ui
    var body: some View { WorkspacePaneInner(workspace: workspace, ps: ui.state(for: workspace.id)) }
}

struct WorkspacePaneInner: View {
    let workspace: Workspace
    @Bindable var ps: PaneState
    @Environment(AppState.self) var state
    @EnvironmentObject var theme: ThemeManager

    @State private var opened: Set<PaneKind> = []

    private var safeWs: String {
        workspace.id.replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: ":", with: "-")
    }

    var body: some View {
        GeometryReader { geo in
            let footerH: CGFloat = 23
            let maxDrawer = max(150, geo.size.height - footerH - 45)
            let drawerH: CGFloat = ps.terms.isEmpty ? 0 : (ps.drawerOpen ? min(ps.drawerHeight, maxDrawer) : 0)
            let contentH = max(0, geo.size.height - drawerH - footerH)
            let active = effectivePane
            VStack(spacing: 0) {
                splitArea(width: geo.size.width, height: contentH, active: active)
                    .frame(width: geo.size.width, height: contentH)
                    .onAppear { opened.insert(active); if ps.agentOpen { opened.insert(.claude) } }
                    .onChange(of: ps.pane) { opened.insert(active) }
                    .onChange(of: ps.agentOpen) { if ps.agentOpen { opened.insert(.claude) } }
                if !ps.terms.isEmpty {
                    TerminalDrawer(terms: $ps.terms, selected: $ps.selTerm, height: $ps.drawerHeight,
                                   maxHeight: maxDrawer, wsKey: workspace.id,
                                   onNew: newTerminal, onClose: { ps.drawerOpen = false })
                        .frame(height: drawerH)
                        .clipped()
                        .opacity(ps.drawerOpen ? 1 : 0)
                }
                bottomBar.frame(height: footerH)
            }
            .frame(width: geo.size.width, height: geo.size.height, alignment: .top)
        }
        .perfTag("WorkspacePane")
    }

    private var effectivePane: PaneKind {
        navDisabled(ps.pane) ? .services : ps.pane
    }

    private var functionKinds: [PaneKind] { PaneKind.allCases.filter { $0 != .claude } }

    private func clampAgent(_ w: Double, _ total: CGFloat) -> CGFloat {
        CGFloat(min(max(320, w), max(320, Double(total) - 320)))
    }

    @ViewBuilder private func splitArea(width: CGFloat, height: CGFloat, active: PaneKind) -> some View {
        let showAgent = ps.agentOpen
        let showFunc = ps.funcVisible || !ps.agentOpen
        let handleW: CGFloat = 6
        let agentW = showFunc ? clampAgent(ps.agentWidth, width) : width
        let funcW = showFunc ? (showAgent ? max(0, width - agentW - handleW) : width) : 0
        ZStack(alignment: .topLeading) {
            if showFunc {
                functionArea(active: active).frame(width: funcW)
            }
            if showAgent && showFunc {
                SplitHandle(axis: .horizontal, value: $ps.agentWidth, min: 320, max: max(320, Double(width) - 320), invert: true)
                    .offset(x: funcW)
            }
            if opened.contains(.claude) {
                agentArea()
                    .frame(width: agentW)
                    .offset(x: showAgent ? (showFunc ? funcW + handleW : 0) : width)
                    .allowsHitTesting(showAgent)
                    .animation(.easeInOut(duration: 0.18), value: ps.agentOpen)
            }
        }
        .frame(width: width, height: height, alignment: .topLeading)
        .clipped()
    }

    @ViewBuilder private func functionArea(active: PaneKind) -> some View {
        ZStack {
            ForEach(functionKinds) { kind in
                if opened.contains(kind), !(workspace.isMain && kind == .review) {
                    paneView(kind, active: active == kind)
                        .opacity(active == kind ? 1 : 0)
                        .allowsHitTesting(active == kind)
                        .zIndex(active == kind ? 1 : 0)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .clipped()
    }

    private func agentArea() -> some View {
        paneView(.claude, active: true)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .clipped()
    }

    @ViewBuilder private func paneView(_ kind: PaneKind, active: Bool) -> some View {
        switch kind {
        case .claude:
            AgentTerminal(branch: workspace.branch, isMain: workspace.isMain, wsKey: workspace.id,
                          onClose: { ps.agentOpen = false; ps.funcVisible = true })
                .id("claude-\(safeWs)")
        case .services:
            ServicesBoard(workspace: workspace, openPane: { ps.pane = $0 }, openTerminal: attachLog,
                          onPrepareMain: { state.showPipeline = true })
        case .git:    PRsBoard(workspace: workspace).id("git-\(safeWs)")
        case .jira:   JiraPane(workspace: workspace)
        case .database: DatabasePane(workspace: workspace).id("db-\(safeWs)")
        case .files:
            FilesPane(workspace: workspace, onAskAgent: { text in
                opened.insert(.claude)
                StreamManager.shared.askClaude(wsKey: workspace.id, text: text)
                ps.agentOpen = true; ps.funcVisible = true
            })
            .id("files-\(safeWs)")
        case .review:
            ReviewPane(workspace: workspace, isActive: active, onAskAgent: { text in
                opened.insert(.claude)
                StreamManager.shared.askClaude(wsKey: workspace.id, text: text)
                ps.agentOpen = true; ps.funcVisible = true
            })
        }
    }

    private var bottomBar: some View {
        ViewThatFits(in: .horizontal) {
            barContent(spread: true)
            ScrollView(.horizontal, showsIndicators: false) { barContent(spread: false) }
        }
        .frame(height: 18)
        .padding(.horizontal, 6).padding(.vertical, 2)
        .background(Theme.bgSoft)
    }

    @ViewBuilder private func barContent(spread: Bool) -> some View {
        HStack(spacing: 1) {
            Button {
                state.toggleSidebar()
            } label: {
                Image(systemName: "sidebar.left").font(.system(size: 11))
                    .foregroundStyle(Theme.fgMuted).frame(width: 24, height: 18)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .keyboardShortcut("b", modifiers: .command)
            .tooltip("Toggle Sidebar", shortcut: "⌘B", align: .topLeading)
            Button { state.openActivity(scope: workspace.id) } label: {
                Image(systemName: "gauge.with.dots.needle.67percent").font(.system(size: 11))
                    .foregroundStyle(Theme.fgMuted).frame(width: 24, height: 18).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .tooltip("Activity (this workspace)", shortcut: "⌘0", align: .topLeading)
            Divider().frame(height: 13).overlay(Theme.borderSoft).padding(.horizontal, 3)
            navBtn(.services, "1", "Services")
            navBtn(.git, "2", "Git")
            navBtn(.jira, "3", "Jira")
            navBtn(.database, "4", "Database")
            navBtn(.review, "5", "Review")
            navBtn(.files, "6", "Files")
            editorBtn
            if spread { Spacer(minLength: 8) } else { Spacer().frame(width: 10) }
            agentToggle
            Divider().frame(height: 13).overlay(Theme.borderSoft).padding(.horizontal, 3)
            terminalToggle
        }
    }

    private var agentToggle: some View {
        Button {
            ps.toggleAgent(); if ps.agentOpen { opened.insert(.claude) }
        } label: {
            Image(systemName: PaneKind.claude.icon).font(.system(size: 11))
                .foregroundStyle(ps.agentOpen ? Theme.accent : Theme.fgMuted)
                .frame(width: 24, height: 18)
                .background(ps.agentOpen ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 5))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .tooltip("Agent", shortcut: "⌘I", align: .topTrailing)
    }

    private func navDisabled(_ kind: PaneKind) -> Bool {
        workspace.isMain && (kind == .jira || kind == .review)
    }

    private func navBtn(_ kind: PaneKind, _ key: KeyEquivalent, _ name: String) -> some View {
        let off = navDisabled(kind)
        let on = ps.pane == kind && ps.funcVisible
        return Button { if !off { withAnimation(.easeInOut(duration: 0.16)) { ps.selectFunc(kind) } } } label: {
            Image(systemName: kind.icon).font(.system(size: 11))
                .foregroundStyle(off ? Theme.dim.opacity(0.4) : (on ? Theme.accent : Theme.fgMuted))
                .frame(width: 24, height: 18)
                .background(on && !off ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 5))
                .contentShape(Rectangle())
                .overlay(alignment: .topTrailing) {
                    if kind == .services, workspace.running > 0 {
                        Circle().fill(Theme.ok).frame(width: 4, height: 4).offset(x: -2, y: 2)
                    }
                }
        }
        .buttonStyle(.plain)
        .disabled(off)
        .keyboardShortcut(key, modifiers: .command)
        .tooltip(off ? "\(name) — not on main"
                 : (kind == .services && workspace.total > 0 ? "Services · \(workspace.running)/\(workspace.total)" : name),
                 shortcut: "⌘\(String(key.character).uppercased())")
    }

    private var editorBtn: some View {
        Button { state.openEditor(workspace) } label: {
            Image(systemName: "square.and.pencil").font(.system(size: 11))
                .foregroundStyle(Theme.fgMuted).frame(width: 24, height: 18)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .keyboardShortcut("e", modifiers: .command)
        .tooltip("Open in editor", shortcut: "⌘E")
    }

    private var terminalToggle: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.16)) {
                if ps.drawerOpen {
                    ps.drawerOpen = false
                } else if ps.terms.isEmpty {
                    newTerminal()
                } else {
                    ps.drawerOpen = true
                }
            }
        } label: {
            Image(systemName: "terminal").font(.system(size: 11))
                .foregroundStyle(ps.drawerOpen ? Theme.accent : Theme.fgMuted)
                .frame(width: 24, height: 18)
                .background(ps.drawerOpen ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 5))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .keyboardShortcut("j", modifiers: .command)
        .tooltip("Terminal", shortcut: "⌘J", align: .topTrailing)
        .background {
            Button("") { withAnimation(.easeInOut(duration: 0.16)) { newTerminal() } }
                .keyboardShortcut("n", modifiers: .command).opacity(0)
        }
    }

    private func newTerminal() { ps.newTerminal() }

    private func runInTerminal(title: String, cmd: String) {
        let t = ps.makeShellTab(autorun: cmd, title: title)
        ps.terms.append(t); ps.selTerm = t.id
        withAnimation(.easeInOut(duration: 0.16)) { ps.drawerOpen = true }
    }

    private func attachLog(_ holder: String) {
        if let existing = ps.terms.first(where: { $0.holder == holder }) {
            ps.selTerm = existing.id
        } else {
            let title = holder.components(separatedBy: "-").last ?? "log"
            let t = TermTab(title: title, holder: holder)
            ps.terms.append(t); ps.selTerm = t.id
        }
        withAnimation(.easeInOut(duration: 0.16)) { ps.drawerOpen = true }
    }
}

struct TerminalDrawer: View {
    @EnvironmentObject var theme: ThemeManager
    @Binding var terms: [TermTab]
    @Binding var selected: UUID?
    @Binding var height: CGFloat
    var maxHeight: CGFloat = 760
    let wsKey: String
    let onNew: () -> Void
    let onClose: () -> Void
    @State private var dragStart: CGFloat?
    @State private var resizing = false
    @State private var confirmKill: TermTab?
    @AppStorage("metalTerminal") private var metalTerminal = true
    @AppStorage("termFontFamily") private var termFontFamily = ""

    var body: some View {
        VStack(spacing: 0) {
            resizeHandle
            HStack(spacing: 2) {
                ForEach(terms) { t in
                    Button { selected = t.id } label: {
                        HStack(spacing: 6) {
                            Image(systemName: killable(t.holder) ? "terminal" : "bolt.horizontal.circle")
                                .font(.system(size: 10))
                            Text(t.title).font(.system(size: 11))
                            Button { requestClose(t) } label: { Image(systemName: "xmark").font(.system(size: 8)) }
                                .buttonStyle(.plain).foregroundStyle(Theme.dim)
                        }
                        .foregroundStyle(selected == t.id ? Theme.fg : Theme.fgMuted)
                        .padding(.horizontal, 10).padding(.vertical, 5)
                        .background(selected == t.id ? Theme.panel3 : .clear, in: RoundedRectangle(cornerRadius: 6))
                    }
                    .buttonStyle(.plain)
                }
                Button(action: onNew) { Image(systemName: "plus").font(.system(size: 11)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).padding(.horizontal, 4)
                Spacer()
                Button(action: onClose) { Image(systemName: "chevron.down").font(.system(size: 11)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
            }
            .padding(.horizontal, 8).padding(.top, 2).padding(.bottom, 6)
            .background(Theme.bgSoft)
            Divider().overlay(Theme.borderSoft)

            if let sel = selected, let t = terms.first(where: { $0.id == sel }) {
                if metalTerminal {
                    MetalTerminalPane(holderName: t.holder, wsKey: wsKey, autorun: t.autorun,
                                      fontSize: 12, fontFamily: termFontFamily, themeMode: theme.mode, onClosed: { removeTab(t) }).id(t.holder)
                } else {
                    TerminalPane(holderName: t.holder, wsKey: wsKey, autorun: t.autorun, themeMode: theme.mode,
                                 onClosed: { removeTab(t) }).id(t.holder)
                }
            } else {
                Theme.bg
            }
        }
        .background(Theme.bg)
        .alert("Close terminal?", isPresented: Binding(get: { confirmKill != nil }, set: { if !$0 { confirmKill = nil } })) {
            Button("Cancel", role: .cancel) { confirmKill = nil }
            Button("Close & kill", role: .destructive) { if let t = confirmKill { confirmKill = nil; killAndClose(t) } }
        } message: {
            Text("A process is still running in this terminal. Closing the tab will kill it.")
        }
    }

    private func killable(_ holder: String) -> Bool {
        holder.hasPrefix("appsh-") || holder.hasPrefix("sh-") || holder.hasPrefix("reposh-")
    }

    private var resizeHandle: some View {
        ZStack {
            Theme.bgSoft
            Capsule().fill(resizing ? Theme.accent : Theme.border).frame(width: 34, height: 3)
        }
        .frame(height: 4)
        .overlay(Rectangle().fill(resizing ? Theme.accent : Theme.borderSoft).frame(height: 1), alignment: .top)
        .contentShape(Rectangle())
        .onHover { if $0 { NSCursor.resizeUpDown.set() } else if !resizing { NSCursor.arrow.set() } }
        .gesture(
            DragGesture(minimumDistance: 0, coordinateSpace: .global)
                .onChanged { v in
                    if dragStart == nil { dragStart = height; resizing = true }
                    height = min(maxHeight, max(150, (dragStart ?? height) - v.translation.height))
                }
                .onEnded { _ in dragStart = nil; resizing = false; NSCursor.arrow.set() }
        )
    }

    private func requestClose(_ t: TermTab) {
        guard killable(t.holder) else { removeTab(t); return }
        Task {
            if await busy(t.holder) { confirmKill = t } else { killAndClose(t) }
        }
    }

    private func busy(_ holder: String) async -> Bool {
        await Task.detached(priority: .userInitiated) { () -> Bool in
            struct R: Decodable { var busy = false }
            let d = PaneStore.busy(holder: holder)
            return (PomJSON.decode(R.self, from: d)?.busy) ?? false
        }.value
    }

    private func killAndClose(_ t: TermTab) {
        let h = t.holder
        PaneStore.kill(paneID: "pty:" + h)
        removeTab(t)
    }

    private func removeTab(_ t: TermTab) {
        terms.removeAll { $0.id == t.id }
        if selected == t.id { selected = terms.last?.id }
        if terms.isEmpty { onClose() }
    }
}

struct Placeholder: View {
    let title: String
    let systemImage: String
    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: systemImage).font(.system(size: 34)).foregroundStyle(Theme.dim)
            Text(title).font(.title3).foregroundStyle(Theme.fgMuted)
            Text("Chưa port sang native — sắp có.").font(.caption).foregroundStyle(Theme.dim)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.bg)
    }
}
