import SwiftUI

struct PomeloApp: App {
    @State private var state = AppState()
    @StateObject private var theme = ThemeManager()
    @State private var ui = UIStore()
    @StateObject private var updater = AppUpdater.shared

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(state)
                .environmentObject(theme)
                .environment(ui)
                .frame(minWidth: 1040, minHeight: 640)
                .overlay(alignment: .topTrailing) { PerfHUDOverlay() }
                .onAppear {
                    state.uiStore = ui; state.themeManager = theme; state.boot(); theme.applyToWindow()
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { state.maybeShowSetupOnFirstRun() }
                }
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) { }
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") { updater.checkForUpdates() }
                    .disabled(!updater.canCheckForUpdates)
            }
            CommandGroup(after: .windowArrangement) { OpenMetalSpikeButton(); MetalTerminalToggle(); MetalTermStatsToggle(); PerfHUDToggle() }
        }

        Window("Create workspace", id: "create-workspace") {
            CreateWorkspaceView()
                .environment(state)
                .environmentObject(theme)
                .environment(ui)
        }
        .windowResizability(.contentSize)
        .defaultSize(width: 540, height: 720)
        .defaultPosition(.center)

        Window("Settings", id: "settings") {
            SettingsView()
                .environment(state)
                .environmentObject(theme)
                .environment(ui)
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentSize)
        .defaultSize(width: 880, height: 600)
        .defaultPosition(.center)

        Window("Metal Terminal Spike", id: "metal-spike") {
            MetalTerminalSpike().frame(minWidth: 760, minHeight: 460).background(.black)
        }
        .defaultSize(width: 900, height: 560)
        .defaultPosition(.center)
    }
}
