import SwiftUI
#if canImport(ActivityKit)
import ActivityKit
#endif

public let pomeloAppGroup = "group.app.pomelo.remote"

public struct AgentWorkspaceLite: Codable, Identifiable, Hashable {
    public var branch: String
    public var title: String
    public var isMain: Bool
    public var state: String
    public var running: Int
    public var total: Int
    public var prCount: Int
    public var id: String { (isMain ? "main:" : "ws:") + branch }

    public init(branch: String, title: String, isMain: Bool, state: String, running: Int, total: Int, prCount: Int) {
        self.branch = branch; self.title = title; self.isMain = isMain
        self.state = state; self.running = running; self.total = total; self.prCount = prCount
    }
}

public struct AgentSnapshot: Codable {
    public var mac: String
    public var updated: Date
    public var workspaces: [AgentWorkspaceLite]
    public var activeCount: Int

    public init(mac: String, updated: Date, workspaces: [AgentWorkspaceLite], activeCount: Int) {
        self.mac = mac; self.updated = updated; self.workspaces = workspaces; self.activeCount = activeCount
    }
}

public enum SharedStore {
    private static let key = "agent_snapshot"
    private static var defaults: UserDefaults? { UserDefaults(suiteName: pomeloAppGroup) }

    public static func save(_ snap: AgentSnapshot) {
        guard let d = try? JSONEncoder().encode(snap) else { return }
        defaults?.set(d, forKey: key)
    }

    public static func load() -> AgentSnapshot? {
        guard let d = defaults?.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(AgentSnapshot.self, from: d)
    }
}

public func agentStateColor(_ state: String) -> Color {
    switch state {
    case "idle":           return Color(.sRGB, red: 0.188, green: 0.820, blue: 0.345)
    case "thinking":       return Color(.sRGB, red: 1.000, green: 0.624, blue: 0.039)
    case "tool_use":       return Color(.sRGB, red: 0.392, green: 0.824, blue: 1.000)
    case "compacting":     return Color(.sRGB, red: 0.749, green: 0.353, blue: 0.949)
    case "awaiting_input": return Color(.sRGB, red: 1.000, green: 0.271, blue: 0.227)
    default:               return Color(.sRGB, red: 0.55, green: 0.55, blue: 0.58)
    }
}

public func agentStateActive(_ state: String) -> Bool {
    ["thinking", "tool_use", "compacting", "awaiting_input"].contains(state)
}

public func agentStateLabel(_ state: String) -> String {
    switch state {
    case "idle":           return "Idle"
    case "thinking":       return "Thinking"
    case "tool_use":       return "Working"
    case "compacting":     return "Compacting"
    case "awaiting_input": return "Needs you"
    default:               return state.isEmpty ? "No agent" : state
    }
}

#if canImport(ActivityKit)
@available(iOS 16.1, *)
public struct PomeloAgentAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        public var activeCount: Int
        public var headline: String
        public var state: String
        public var sessionPct: Int
        public var weeklyPct: Int
        public init(activeCount: Int, headline: String, state: String, sessionPct: Int = 0, weeklyPct: Int = 0) {
            self.activeCount = activeCount; self.headline = headline; self.state = state
            self.sessionPct = sessionPct; self.weeklyPct = weeklyPct
        }
    }
    public var mac: String
    public init(mac: String) { self.mac = mac }
}
#endif
