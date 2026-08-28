import SwiftUI

struct SetupWizard: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    var onClose: () -> Void = {}

    private let steps = ["Notifications", "Claude MCP"]
    @State private var step = 0
    @State private var notifOK = false
    @State private var mcpRegistered = false
    @State private var mcpWrapper = false
    @State private var mcpConnected = false
    @State private var busy = false

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.borderSoft)
            ScrollView { stepBody.padding(24).frame(maxWidth: .infinity, alignment: .leading) }
            Divider().overlay(Theme.borderSoft)
            footer
        }
        .frame(width: 560, height: 440)
        .background(Theme.bg)
        .task { await refresh() }
        .onExitCommand { onClose() }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "sparkles").font(.system(size: 14)).foregroundStyle(Theme.accent)
            Text("Set up Pomelo").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            Spacer()
            HStack(spacing: 6) {
                ForEach(steps.indices, id: \.self) { i in
                    Capsule().fill(i == step ? Theme.accent : (i < step ? Theme.ok : Theme.dim.opacity(0.4)))
                        .frame(width: i == step ? 18 : 8, height: 6)
                }
            }
            Button { onClose() } label: { Image(systemName: "xmark").font(.system(size: 12)) }
                .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).padding(.leading, 6)
        }
        .padding(.horizontal, 18).padding(.vertical, 12)
    }

    @ViewBuilder private var stepBody: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Step \(step + 1) of \(steps.count) · \(steps[step])")
                .font(.system(size: 11, weight: .semibold)).kerning(0.5).foregroundStyle(Theme.muted)
            if step == 0 { notifStep } else { mcpStep }
        }
    }

    private var notifStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Native macOS notifications").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            Text("Get pinged when Claude finishes, needs input, or a workspace needs attention — even when Pomelo is in the background.")
                .font(.system(size: 12.5)).foregroundStyle(Theme.fgMuted).fixedSize(horizontal: false, vertical: true)
            statusRow("Notifications allowed", ok: notifOK)
            HStack(spacing: 8) {
                Button { Notifier.promptOrOpenSettings(); recheckSoon() } label: { Label(notifOK ? "Open settings" : "Enable notifications", systemImage: "bell.badge") }
                    .buttonStyle(.borderedProminent).tint(Theme.accent).controlSize(.small)
                Button { Notifier.sendTest() } label: { Label("Send test", systemImage: "paperplane") }
                    .buttonStyle(.bordered).controlSize(.small)
            }
        }
    }

    private var mcpStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Claude MCP").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            Text("Pomelo registers a global `pom` MCP so Claude (in the terminal and in-app) can inspect and act on your workspace — ports, services, config, databases.")
                .font(.system(size: 12.5)).foregroundStyle(Theme.fgMuted).fixedSize(horizontal: false, vertical: true)
            statusRow("Registered in ~/.claude.json", ok: mcpRegistered)
            statusRow("Wrapper script present", ok: mcpWrapper)
            statusRow("Connected", ok: mcpConnected)
            HStack(spacing: 8) {
                Button { Task { await reregister() } } label: { Label("Re-register", systemImage: "wrench.and.screwdriver") }
                    .buttonStyle(.borderedProminent).tint(Theme.accent).controlSize(.small).disabled(busy)
                Button { Task { await refresh() } } label: { Label("Recheck", systemImage: "arrow.clockwise") }
                    .buttonStyle(.bordered).controlSize(.small).disabled(busy)
                if busy { Spinner() }
            }
        }
    }

    private func statusRow(_ label: String, ok: Bool) -> some View {
        HStack(spacing: 8) {
            Image(systemName: ok ? "checkmark.circle.fill" : "circle").foregroundStyle(ok ? Theme.ok : Theme.dim).font(.system(size: 13))
            Text(label).font(.system(size: 12.5)).foregroundStyle(Theme.fg)
        }
    }

    private var footer: some View {
        HStack {
            if step > 0 { Button("Back") { step -= 1 } .buttonStyle(.bordered).controlSize(.small) }
            Spacer()
            if step < steps.count - 1 {
                Button("Next") { step += 1; Task { await refresh() } }.buttonStyle(.borderedProminent).tint(Theme.accent).controlSize(.small)
            } else {
                Button("Finish") { onClose() }.buttonStyle(.borderedProminent).tint(Theme.accent).controlSize(.small)
            }
        }
        .padding(.horizontal, 18).padding(.vertical, 12)
    }

    private func recheckSoon() { DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { Task { await refresh() } } }

    private func refresh() async {
        Notifier.currentlyAuthorized { notifOK = $0 }
        let r = await MCPViewModel.fetchStatus()
        mcpRegistered = r.registered; mcpConnected = r.connected; mcpWrapper = r.wrapper_ok
    }

    private func reregister() async {
        busy = true
        await MCPViewModel.doReinstall()
        busy = false
        await refresh()
    }
}
