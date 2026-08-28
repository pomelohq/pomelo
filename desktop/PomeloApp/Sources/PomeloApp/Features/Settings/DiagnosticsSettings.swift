import SwiftUI
import AppKit

struct DiagnosticsSettings: View {
    enum Tab: String, CaseIterable, Identifiable { case log, proxy, mcp
        var id: String { rawValue }
        var title: String { switch self { case .log: return "App log"; case .proxy: return "Dev-proxy"; case .mcp: return "MCP" } }
        var icon: String { switch self { case .log: return "ladybug"; case .proxy: return "arrow.triangle.branch"; case .mcp: return "sparkles" } }
    }
    @State private var tab: Tab = .log

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $tab) {
                ForEach(Tab.allCases) { t in Label(t.title, systemImage: t.icon).tag(t) }
            }
            .pickerStyle(.segmented).labelsHidden()
            .padding(.horizontal, 24).padding(.top, 4).padding(.bottom, 12)
            Divider().overlay(Theme.borderSoft)
            switch tab {
            case .log: AppLogPane()
            case .proxy: ProxyLogPane()
            case .mcp: MCPPane()
            }
        }
    }
}

private struct MCPPane: View {
    @StateObject private var vm = MCPViewModel()

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if vm.loading {
                    HStack(spacing: 8) { Spinner(); Text("Checking claude mcp…").font(.system(size: 12)).foregroundStyle(Theme.fgMuted) }
                } else {
                    statusRow("Registered in ~/.claude.json", ok: vm.status.registered)
                    statusRow("Wrapper script present", ok: vm.status.wrapper_ok)
                    statusRow("Connected (claude mcp list)", ok: vm.status.connected)
                    if !vm.status.command.isEmpty {
                        VStack(alignment: .leading, spacing: 3) {
                            SectionLabel(text: "COMMAND", size: 10)
                            Text(vm.status.command).font(Theme.mono(11)).foregroundStyle(Theme.fg).textSelection(.enabled)
                        }
                    }
                    if !vm.status.list_line.isEmpty {
                        Text(vm.status.list_line).font(Theme.mono(10.5)).foregroundStyle(Theme.fgMuted).textSelection(.enabled)
                    }
                    Text("The pom MCP is registered globally so `claude mcp list` and every Claude session (terminal + the in-app work Claude) can use it, resolving the session from the working directory.")
                        .font(.system(size: 11)).foregroundStyle(Theme.fgMuted).fixedSize(horizontal: false, vertical: true)
                }
                HStack(spacing: 8) {
                    Button { Task { await vm.load() } } label: { Label("Recheck", systemImage: "arrow.clockwise") }
                        .buttonStyle(.bordered).controlSize(.small).disabled(vm.busy || vm.loading)
                    Button { Task { await vm.reinstall() } } label: { Label("Re-register", systemImage: "wrench.and.screwdriver") }
                        .buttonStyle(.borderedProminent).tint(Theme.accent).controlSize(.small).disabled(vm.busy)
                    if vm.busy { Spinner() }
                    Spacer()
                }
            }
            .padding(24).frame(maxWidth: .infinity, alignment: .leading)
        }
        .task { await vm.load() }
    }

    private func statusRow(_ label: String, ok: Bool) -> some View {
        HStack(spacing: 8) {
            Image(systemName: ok ? "checkmark.circle.fill" : "xmark.circle.fill")
                .foregroundStyle(ok ? Theme.ok : Theme.danger).font(.system(size: 13))
            Text(label).font(.system(size: 12.5)).foregroundStyle(Theme.fg)
        }
    }
}

private struct AppLogPane: View {
    @StateObject private var vm = LogsViewModel()
    @State private var copied = false
    private var os: String { ProcessInfo.processInfo.operatingSystemVersionString }

    var body: some View {
        VStack(spacing: 0) {
            if vm.loading {
                spinner("loading logs…")
            } else if vm.isEmpty {
                empty("doc.text.magnifyingglass", "No logs yet")
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        Text(vm.lines.joined(separator: "\n"))
                            .font(Theme.mono(10.5)).foregroundStyle(Theme.fg)
                            .frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled)
                            .padding(12).id("end")
                    }
                    .background(Theme.bg)
                    .onAppear { proxy.scrollTo("end", anchor: .bottom) }
                }
            }
            Divider().overlay(Theme.borderSoft)
            HStack(spacing: 10) {
                Text(vm.loading ? "…" : "Pomelo \(vm.version) · \(vm.session)")
                    .font(.system(size: 11)).foregroundStyle(Theme.dim).lineLimit(1)
                Spacer()
                IconButton("arrow.clockwise", tip: "Refresh") { Task { await vm.load() } }
                if !vm.logfile.isEmpty {
                    Button { NSWorkspace.shared.selectFile(vm.logfile, inFileViewerRootedAtPath: "") } label: {
                        Label("Reveal app.log", systemImage: "folder").font(.system(size: 12))
                    }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
                }
                Button { copyDiagnostics() } label: {
                    HStack(spacing: 5) {
                        Image(systemName: copied ? "checkmark" : "doc.on.doc").font(.system(size: 11))
                        Text(copied ? "Copied" : "Copy diagnostics").font(.system(size: 12, weight: .medium))
                    }
                }.buttonStyle(.borderedProminent).tint(Theme.accent)
            }
            .padding(.horizontal, 18).padding(.vertical, 12)
        }
        .task { await vm.load() }
    }

    private func copyDiagnostics() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(vm.diagnostics(os: os), forType: .string)
        copied = true
        Task { try? await Task.sleep(nanoseconds: 1_500_000_000); copied = false }
    }
}

private struct ProxyLogPane: View {
    @StateObject private var vm = ProxyLogViewModel()
    @State private var timer: Timer?
    private var entries: [ProxyLogEntry] { vm.entries }

    var body: some View {
        Group {
            if entries.isEmpty {
                VStack(spacing: 6) {
                    Image(systemName: "arrow.triangle.branch").font(.system(size: 28)).foregroundStyle(Theme.dim)
                    Text("No proxy requests yet").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                    Text("Hit a /_pom_dev/<repo>/<service> route from a frontend to see where it goes.")
                        .font(.system(size: 11)).foregroundStyle(Theme.dim).multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity).padding(24)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(entries) { e in row(e); Divider().overlay(Theme.borderSoft.opacity(0.5)) }
                    }
                }.background(Theme.bg)
            }
        }
        .onAppear {
            Task { await vm.refresh() }
            timer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { _ in Task { await vm.refresh() } }
        }
        .onDisappear { timer?.invalidate(); timer = nil }
    }

    private func row(_ e: ProxyLogEntry) -> some View {
        HStack(spacing: 10) {
            Text(e.time).font(Theme.mono(10.5)).foregroundStyle(Theme.dim).frame(width: 62, alignment: .leading)
            Text(e.method).font(Theme.mono(10.5, .semibold)).foregroundStyle(Theme.fgMuted).frame(width: 42, alignment: .leading)
            Text(e.path).font(Theme.mono(11)).foregroundStyle(Theme.fg).lineLimit(1).frame(width: 190, alignment: .leading)
            Text(e.profile).font(Theme.mono(10.5, .semibold))
                .foregroundStyle(e.isRemote ? Theme.warn : Theme.ok)
                .padding(.horizontal, 6).padding(.vertical, 1)
                .background((e.isRemote ? Theme.warn : Theme.ok).opacity(0.15), in: Capsule())
            Image(systemName: "arrow.right").font(.system(size: 8)).foregroundStyle(Theme.dim)
            Text(e.target).font(Theme.mono(10.5)).foregroundStyle(e.isRemote ? Theme.warn : Theme.fgMuted)
                .lineLimit(1).truncationMode(.middle).frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled)
            Text("\(e.status)").font(Theme.mono(10.5)).foregroundStyle(e.status >= 400 ? Theme.danger : Theme.fgMuted)
            Text("\(e.ms)ms").font(Theme.mono(10)).foregroundStyle(Theme.dim).frame(width: 52, alignment: .trailing)
        }
        .padding(.horizontal, 18).padding(.vertical, 6)
    }
}

private func spinner(_ label: String) -> some View {
    LoadingView(text: label)
}
private func empty(_ icon: String, _ label: String) -> some View {
    VStack(spacing: 8) {
        Image(systemName: icon).font(.system(size: 30)).foregroundStyle(Theme.dim)
        Text(label).font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
    }.frame(maxWidth: .infinity, maxHeight: .infinity)
}
