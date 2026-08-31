import SwiftUI
import UIKit

struct AgentView: View {
    @EnvironmentObject var theme: ThemeManager
    let client: RemoteClient
    let workspace: WorkspaceRow

    @StateObject private var ctl = TerminalController()
    @State private var window = ""
    @State private var resolving = true
    @State private var draft = ""
    @State private var loadError = ""
    @State private var kbHeight: CGFloat = 0
    @State private var restartToken = 0
    @FocusState private var inputFocused: Bool
    @AppStorage("claudeFontSize") private var fontSize: Double = 12

    var body: some View {
        ZStack {
            Theme.bg.ignoresSafeArea()
            VStack(spacing: 0) {
                if resolving {
                    ProgressView().tint(Theme.accent).frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if window.isEmpty {
                    VStack(spacing: 8) {
                        Image(systemName: "terminal").font(.system(size: 28)).foregroundStyle(Theme.fgMuted)
                        Text(loadError.isEmpty ? "No agent terminal for this workspace" : loadError)
                            .font(Theme.ui(12)).foregroundStyle(Theme.fgMuted).multilineTextAlignment(.center)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    agentHeader
                    if ctl.ended {
                        endedView
                    } else {
                        PtyTerminalView(client: client, window: window, ctl: ctl, fontSize: CGFloat(fontSize))
                            .id(restartToken)
                            .contentShape(Rectangle())
                            .onTapGesture { inputFocused = false }
                        keyBar
                        messageBar
                    }
                }
            }
            .offset(y: -max(0, kbHeight - safeBottom))
            .animation(.easeOut(duration: 0.22), value: kbHeight)
        }
        .ignoresSafeArea(.keyboard, edges: .bottom)
        .task { await resolve() }
        .onDisappear { ctl.stop() }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) { n in
            guard let f = n.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect else { return }
            kbHeight = max(0, UIScreen.main.bounds.height - f.origin.y)
        }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in
            kbHeight = 0
        }
    }

    private var safeBottom: CGFloat {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap { $0.windows }
            .first(where: { $0.isKeyWindow })?.safeAreaInsets.bottom ?? 0
    }

    private func resolve() async {
        do {
            window = try await client.claudeTerminalWindow(branch: workspace.branch, isMain: workspace.isMain)
        } catch let e { loadError = describe(e) }
        resolving = false
    }

    private var agentHeader: some View {
        HStack(spacing: 10) {
            Text("agent · \(workspace.branch)").font(Theme.mono(12)).foregroundStyle(Theme.fgMuted)
                .lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 6)
            Button { fontSize = max(9, fontSize - 1) } label: { Text("A-").font(Theme.ui(13)).foregroundStyle(Theme.fgMuted) }
                .buttonStyle(.plain)
            Button { fontSize = min(22, fontSize + 1) } label: { Text("A+").font(Theme.ui(14)).foregroundStyle(Theme.fgMuted) }
                .buttonStyle(.plain)
            Button { ctl.close() } label: {
                HStack(spacing: 3) { Image(systemName: "xmark").font(.system(size: 9)); Text("Close").font(Theme.ui(12)) }
                    .foregroundStyle(Theme.fgMuted)
            }.buttonStyle(.plain)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(Theme.bgSoft)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.borderSoft).frame(height: 1) }
    }

    private var endedView: some View {
        VStack(spacing: 10) {
            Text("Agent session ended").font(Theme.ui(13)).foregroundStyle(Theme.fgMuted)
            Button { restart() } label: {
                HStack(spacing: 5) {
                    Image(systemName: "arrow.clockwise").font(.system(size: 11))
                    Text("Restart agent").font(Theme.ui(12, .semibold))
                }
                .foregroundStyle(Theme.accent).padding(.horizontal, 14).padding(.vertical, 7)
                .background(Theme.accentSoft, in: Capsule())
            }.buttonStyle(.plain)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.bg)
    }

    private func restart() {
        Task {
            resolving = true
            await resolve()
            restartToken += 1
            ctl.ended = false
        }
    }

    private var keyBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                key("esc") { ctl.input([0x1b]) }
                key("tab") { ctl.input([0x09]) }
                key("⌃C") { ctl.input([0x03]) }
                key("⌃D") { ctl.input([0x04]) }
                key("⌃Z") { ctl.input([0x1a]) }
                key("←") { ctl.input([0x1b, 0x5b, 0x44]) }
                key("↑") { ctl.input([0x1b, 0x5b, 0x41]) }
                key("↓") { ctl.input([0x1b, 0x5b, 0x42]) }
                key("→") { ctl.input([0x1b, 0x5b, 0x43]) }
                key("PgUp") { ctl.input([0x1b, 0x5b, 0x35, 0x7e]) }
                key("PgDn") { ctl.input([0x1b, 0x5b, 0x36, 0x7e]) }
                key("Home") { ctl.input([0x1b, 0x5b, 0x48]) }
                key("End") { ctl.input([0x1b, 0x5b, 0x46]) }
                key("⏎") { ctl.input([0x0d]) }
            }
            .padding(.horizontal, 8).padding(.vertical, 6)
        }
        .background(Theme.bgSoft)
        .overlay(alignment: .top) { Rectangle().fill(Theme.borderSoft).frame(height: 1) }
    }

    private func key(_ label: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label).font(Theme.mono(13, .medium)).foregroundStyle(Theme.fg)
                .frame(minWidth: 34, minHeight: 30)
                .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 7))
                .overlay(RoundedRectangle(cornerRadius: 7).stroke(Theme.borderSoft, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    private var messageBar: some View {
        HStack(spacing: 8) {
            TextField("Send a message to the agent...", text: $draft, axis: .vertical)
                .font(Theme.mono(12)).foregroundStyle(Theme.fg).lineLimit(1...4)
                .focused($inputFocused)
                .padding(8)
                .background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.borderSoft, lineWidth: 1))
                .toolbar {
                    ToolbarItemGroup(placement: .keyboard) {
                        Spacer()
                        Button { inputFocused = false } label: {
                            Image(systemName: "keyboard.chevron.compact.down")
                        }
                    }
                }
            Button {
                let t = draft.trimmingCharacters(in: .whitespacesAndNewlines)
                if !t.isEmpty { ctl.send(t); draft = "" }
            } label: {
                Image(systemName: "paperplane.fill").foregroundStyle(draft.isEmpty ? Theme.dim : Theme.accent)
            }
            .disabled(draft.isEmpty)
        }
        .padding(10)
        .background(Theme.bgSoft)
    }
}
