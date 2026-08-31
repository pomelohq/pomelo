import Foundation

struct WsAgent: Decodable, Hashable {
    var name = ""
    var state = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? ""
    }
    enum K: String, CodingKey { case name, state }
}

struct WorkspaceRow: Decodable, Identifiable, Hashable {
    var branch = ""
    var displayName = ""
    var isMain = false
    var running = 0
    var total = 0
    var agents: [WsAgent] = []
    var id: String { (isMain ? "main:" : "") + branch }
    var title: String { displayName.isEmpty ? branch : displayName }

    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        branch = try c.decodeIfPresent(String.self, forKey: .branch) ?? ""
        displayName = try c.decodeIfPresent(String.self, forKey: .display_name) ?? ""
        isMain = try c.decodeIfPresent(Bool.self, forKey: .is_main) ?? false
        running = try c.decodeIfPresent(Int.self, forKey: .running) ?? 0
        total = try c.decodeIfPresent(Int.self, forKey: .total) ?? 0
        agents = try c.decodeIfPresent([WsAgent].self, forKey: .agents) ?? []
    }
    enum K: String, CodingKey { case branch, display_name, is_main, running, total, agents }
}

struct WorkspacesPayload: Decodable { var workspaces: [WorkspaceRow] = [] }

struct AgentFrame: Decodable {
    var type = ""
    var text = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        type = try c.decodeIfPresent(String.self, forKey: .type) ?? ""
        let t1 = try c.decodeIfPresent(String.self, forKey: .text)
        let t2 = try c.decodeIfPresent(String.self, forKey: .v)
        let t3 = try c.decodeIfPresent(String.self, forKey: .content)
        text = t1 ?? t2 ?? t3 ?? ""
    }
    enum K: String, CodingKey { case type, text, v, content }
}

enum PomJSON {
    static func decode<T: Decodable>(_ type: T.Type, from data: Data) -> T? {
        try? JSONDecoder().decode(type, from: data)
    }
}
