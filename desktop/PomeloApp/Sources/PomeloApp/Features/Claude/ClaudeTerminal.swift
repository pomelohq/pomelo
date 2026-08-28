import SwiftUI

struct ClaudeTerminal: View {
    @EnvironmentObject var state: AppState
    let branch: String
    let isMain: Bool
    let wsKey: String
    var onClose: () -> Void = {}

    @StateObject private var vm = ClaudeTerminalViewModel()
    @State private var holder: String?
    @AppStorage("claudeFontSize") private var fontSize: Double = 12
    @State private var failed = false
    @State private var exited = false
    @State private var openedAt = Date()
    @State private var autoRetried = false
    @State private var slowStart = false

    private func onHolderClosed() {
        let fast = Date().timeIntervalSince(openedAt) < 6
        if fast && !autoRetried {
            autoRetried = true; holder = nil
            Task { try? await Task.sleep(nanoseconds: 500_000_000); await resolve() }
            return
        }
        exited = true; holder = nil
    }

    private var agent: String? { state.agentStates[wsKey] }
    private var agentColor: Color {
        switch agent {
        case "idle":           return Theme.ok
        case "thinking":       return Theme.warn
        case "tool_use":       return Theme.tool
        case "compacting":     return Theme.wsAccent
        case "awaiting_input": return Theme.danger
        default:               return Theme.ok
        }
    }
    private var agentActive: Bool { agent == "thinking" || agent == "tool_use" || agent == "awaiting_input" }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.borderSoft)
            if exited {
                VStack(spacing: 10) {
                    Text("Claude session ended").font(.system(size: 12.5)).foregroundStyle(Theme.fgMuted)
                    Button { restart() } label: {
                        HStack(spacing: 5) { Image(systemName: "arrow.clockwise").font(.system(size: 11)); Text("Restart Claude").font(.system(size: 12, weight: .medium)) }
                            .foregroundStyle(Theme.accent).padding(.horizontal, 12).padding(.vertical, 5)
                            .background(Theme.accentSoft, in: Capsule())
                    }.buttonStyle(.plain)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity).background(Theme.bg)
            } else if let h = holder {
                TerminalPane(holderName: h, wsKey: wsKey, fontSize: CGFloat(fontSize),
                             onClosed: { onHolderClosed() }).id(h)
            } else if failed {
                Text("Could not start Claude").font(.system(size: 12)).foregroundStyle(Theme.danger)
                    .frame(maxWidth: .infinity, maxHeight: .infinity).background(Theme.bg)
            } else {
                // Delay the spinner: re-attaching to an existing session resolves in
                // a few ms, so don't flash "starting claude…" on every workspace switch.
                VStack(spacing: 8) {
                    if slowStart {
                        Spinner()
                        Text("starting claude…").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity).background(Theme.bg)
                .task {
                    try? await Task.sleep(nanoseconds: 350_000_000)
                    if holder == nil && !failed && !exited { slowStart = true }
                }
            }
        }
        .task { await resolve() }
    }

    private func restart() {
        exited = false; failed = false; autoRetried = false; holder = nil
        Task { await resolve() }
    }

    private var header: some View {
        HStack(spacing: 8) {
            AgentOrb(color: agentColor, active: agentActive, size: 8)
            Text("claude · \(branch)").font(Theme.mono(12)).foregroundStyle(Theme.fgMuted)
            Spacer()
            Button { shrinkFont() } label: { Text("A-").font(.system(size: 12)) }
                .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).help("Smaller (⌘−)")
                .keyboardShortcut("-", modifiers: .command)
            Button { growFont() } label: { Text("A+").font(.system(size: 12)) }
                .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).help("Bigger (⌘+)")
                .keyboardShortcut("=", modifiers: .command)
            Button { closeSession() } label: {
                HStack(spacing: 4) { Image(systemName: "xmark").font(.system(size: 9)); Text("Close").font(.system(size: 12)) }
                    .foregroundStyle(Theme.fgMuted)
            }
            .buttonStyle(.plain)
            .keyboardShortcut("w", modifiers: [.command, .shift])
            .help("Close Claude (⌘⇧W)")
            Button { growFont() } label: { EmptyView() }
                .buttonStyle(.plain).frame(width: 0, height: 0).opacity(0)
                .keyboardShortcut("+", modifiers: .command)
        }
        .padding(.horizontal, 12).padding(.vertical, 7)
        .background(Theme.bgSoft)
    }

    private func shrinkFont() { fontSize = max(9, fontSize - 1) }
    private func growFont() { fontSize = min(22, fontSize + 1) }

    private func closeSession() {
        if let h = holder { vm.kill(paneID: h) }
        onClose()
    }

    private func resolve() async {
        let win = await vm.resolveWindow(branch: branch, isMain: isMain)
        if let win, !win.isEmpty { openedAt = Date(); holder = win } else { failed = true }
    }
}
