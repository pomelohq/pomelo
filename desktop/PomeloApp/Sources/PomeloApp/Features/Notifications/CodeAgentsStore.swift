import Foundation

// The code agents (Claude, ...) advertised by the core, used to build the
// notification-sound catalog. FFI seam per ADR 0001.
enum CodeAgentsStore {
    struct Agent: Decodable { var id = ""; var name = "" }

    static func load() async -> [(id: String, name: String)] {
        let d = await Task.detached(priority: .utility) { PomCore.shared.codeAgentsData() }.value
        return (PomJSON.decode([Agent].self, from: d) ?? []).map { (id: $0.id, name: $0.name) }
    }
}
