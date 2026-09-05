import Foundation

// Claude usage snapshot decoded from the core's `claude_usage` domain. Shared by both
// apps (macOS usage chip, iOS usage bars + Live Activity). Optionals: Go omits fields
// on success (omitempty), so non-optional fields would make synthesized Decodable throw.
public struct ClaudeUsage: Decodable, Equatable {
    public struct Win: Decodable, Equatable {
        public var pct: Double?
        public var resets_at: Int64?
    }
    public struct Account: Decodable, Equatable {
        public var email: String?
        public var plan: String?
        public var org: String?
    }
    public var ok: Bool?
    public var error: String?
    public var session: Win?
    public var weekly: Win?
    public var account: Account?
}
