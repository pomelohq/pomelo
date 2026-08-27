import Foundation

// Owns the MCP status pane state; the View renders it (ADR 0001).
@MainActor
final class MCPViewModel: ObservableObject {
    struct Status: Decodable {
        var registered = false, connected = false, wrapper_ok = false
        var command = "", list_line = ""
    }
    @Published private(set) var status = Status()
    @Published private(set) var loading = true
    @Published private(set) var busy = false

    // Stateless FFI wrappers, reused by other MCP-status views (e.g. SetupWizard).
    static func fetchStatus() async -> Status {
        let d = await Task.detached(priority: .utility) { PomCore.shared.mcpStatusData() }.value
        return PomJSON.decode(Status.self, from: d) ?? Status()
    }
    static func doReinstall() async {
        _ = await Task.detached(priority: .utility) { PomCore.shared.mcpReinstallData() }.value
    }

    func load() async {
        loading = true
        status = await Self.fetchStatus()
        loading = false
    }

    func reinstall() async {
        busy = true
        await Self.doReinstall()
        busy = false
        await load()
    }
}

// Owns the dev-proxy request log; polled off-main by the pane's timer.
@MainActor
final class ProxyLogViewModel: ObservableObject {
    struct Payload: Decodable { var entries: [ProxyLogEntry] }
    @Published private(set) var entries: [ProxyLogEntry] = []

    func refresh() async {
        let d = await Task.detached(priority: .utility) { PomCore.shared.devProxyLogData(limit: 80) }.value
        if let r = PomJSON.decode(Payload.self, from: d) { entries = r.entries }
    }
}
