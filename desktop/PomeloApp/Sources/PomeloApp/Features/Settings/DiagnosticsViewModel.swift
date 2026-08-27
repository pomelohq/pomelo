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

    func load() async {
        loading = true
        let d = await Task.detached(priority: .utility) { PomCore.shared.mcpStatusData() }.value
        if let s = PomJSON.decode(Status.self, from: d) { status = s }
        loading = false
    }

    func reinstall() async {
        busy = true
        _ = await Task.detached(priority: .utility) { PomCore.shared.mcpReinstallData() }.value
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
