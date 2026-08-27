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
            .overlay(TooltipOverlay().zIndex(2000))
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
            .sheet(item: $state.updateMainWs) { ws in
                UpdateMainSheet(ws: ws).environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: $state.showShared) {
                SharedServicesView(onClose: { state.showShared = false }).environmentObject(state).environmentObject(theme)
            }
            .sheet(isPresented: $state.showDependencies) {
                DependencyBoard(onClose: { state.showDependencies = false }).environmentObject(state).environmentObject(theme)
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
                if let m = state.onboardModel {
                    OnboardSheet(model: m, startAt: state.onboardStartAt ?? Date(), branch: state.onboardBranchName,
                                 onBackground: { state.onboardBranch = nil },
                                 onDone: { state.endOnboard() })
                        .environmentObject(state).environmentObject(theme)
                }
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
                        } else if state.selectedWorkspace != nil {
                            KeepAliveWorkspaceHost()
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
