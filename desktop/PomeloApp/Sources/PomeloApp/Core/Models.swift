import Foundation

struct WorkspacesResponse: Decodable {
    let workspaces: [Workspace]
}

struct SessionsResponse: Decodable {
    let sessions: [SessionItem]
    let current: String?
}

struct SessionItem: Decodable, Identifiable, Equatable {
    let name: String
    let path: String?
    let current: Bool
    let running: Bool
    let available: Bool?
    var isAvailable: Bool { available ?? true }
    var id: String { name }
}

struct Workspace: Decodable, Identifiable, Equatable {
    let branch: String
    let displayName: String?
    let isMain: Bool
    let path: String
    var repos: [Repo]
    var wsServices: [Service]?
    var running: Int
    var total: Int

    var id: String { (isMain ? "main:" : "ws:") + branch }
    var title: String { displayName ?? branch }
    var claudeAgentState: String? { wsServices?.compactMap { $0.agentState }.first }

    enum CodingKeys: String, CodingKey {
        case branch, path, repos, running, total
        case wsServices = "ws_services"
        case displayName = "display_name"
        case isMain = "is_main"
    }
}

struct Repo: Decodable, Identifiable, Equatable {
    let name: String
    let alias: String?
    let path: String
    let branch: String
    let dirty: Int
    var services: [Service]?
    let shortcuts: [Shortcut]?

    var id: String { name }
}

struct Service: Decodable, Identifiable, Equatable {
    let name: String
    var running: Bool
    let mode: String?
    let modes: [String]?
    var port: Int?
    let env: String?
    let profiles: [String]?
    let tmuxWindow: String?
    let agentName: String?
    var agentState: String?
    var crashed: Bool?
    var crashLog: String?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, running, mode, modes, port, env, profiles, crashed
        case tmuxWindow = "tmux_window"
        case agentName = "agent_name"
        case agentState = "agent_state"
        case crashLog = "crash_log"
    }
}

struct Shortcut: Decodable, Identifiable, Equatable {
    let cmd: String
    let desc: String
    let key: String?
    var id: String { key ?? cmd }
}
