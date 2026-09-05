import Foundation

// Workspace model graph shared by both apps. The core returns a full payload over the
// in-process FFI (macOS: path/repos/ws_services) and a lighter one over the network
// (iOS: branch/display_name/running/total/agents). A tolerant decoder with defaults
// accepts both, so every field is non-optional-with-default and existing call sites on
// either platform are unchanged.

public struct WorkspacesResponse: Decodable {
    public var workspaces: [Workspace]
    public init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        workspaces = try c.decodeIfPresent([Workspace].self, forKey: .workspaces) ?? []
    }
    enum CodingKeys: String, CodingKey { case workspaces }
}

public struct Workspace: Decodable, Identifiable, Hashable {
    public var branch: String
    public var displayName: String?
    public var isMain: Bool
    public var path: String
    public var repos: [Repo]
    public var wsServices: [Service]?
    public var running: Int
    public var total: Int
    public var agents: [WsAgent]

    public var id: String { (isMain ? "main:" : "ws:") + branch }
    public var title: String { let n = displayName ?? ""; return n.isEmpty ? branch : n }
    public var claudeAgentState: String? { wsServices?.compactMap { $0.agentState }.first }

    enum CodingKeys: String, CodingKey {
        case branch, path, repos, running, total, agents
        case wsServices = "ws_services"
        case displayName = "display_name"
        case isMain = "is_main"
    }

    public init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        branch = try c.decodeIfPresent(String.self, forKey: .branch) ?? ""
        displayName = try c.decodeIfPresent(String.self, forKey: .displayName)
        isMain = try c.decodeIfPresent(Bool.self, forKey: .isMain) ?? false
        path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
        repos = try c.decodeIfPresent([Repo].self, forKey: .repos) ?? []
        wsServices = try c.decodeIfPresent([Service].self, forKey: .wsServices)
        running = try c.decodeIfPresent(Int.self, forKey: .running) ?? 0
        total = try c.decodeIfPresent(Int.self, forKey: .total) ?? 0
        agents = try c.decodeIfPresent([WsAgent].self, forKey: .agents) ?? []
    }
}

public struct WsAgent: Decodable, Hashable {
    public var name: String
    public var state: String
    enum CodingKeys: String, CodingKey { case name, state }
    public init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? ""
    }
}

public struct Repo: Decodable, Identifiable, Hashable {
    public let name: String
    public let alias: String?
    public let path: String
    public let branch: String
    public let dirty: Int
    public var services: [Service]?
    public let shortcuts: [Shortcut]?
    public var id: String { name }
}

public struct Service: Decodable, Identifiable, Hashable {
    public let name: String
    public var running: Bool
    public let mode: String?
    public let modes: [String]?
    public var port: Int?
    public let env: String?
    public let profiles: [String]?
    public let tmuxWindow: String?
    public let agentName: String?
    public var agentState: String?
    public var crashed: Bool?
    public var crashLog: String?

    public var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, running, mode, modes, port, env, profiles, crashed
        case tmuxWindow = "tmux_window"
        case agentName = "agent_name"
        case agentState = "agent_state"
        case crashLog = "crash_log"
    }
}

public struct Shortcut: Decodable, Identifiable, Hashable {
    public let cmd: String
    public let desc: String
    public let key: String?
    public var id: String { key ?? cmd }
}
