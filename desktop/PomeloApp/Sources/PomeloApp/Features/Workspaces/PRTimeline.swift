import Foundation

// Render-ready timeline built by the Go core (internal/provider/forge Timeline).
struct PRTimelineItem: Decodable, Identifiable, Equatable {
    let id: String
    let kind: String            // description | comment | review | inline
    var author: String? = nil
    var avatar: String? = nil
    var body: String = ""
    var at: String = ""
    var state: String = ""
    var resolved: Bool = false
    var threads: [PRThread] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decode(String.self, forKey: .id)
        kind = try c.decode(String.self, forKey: .kind)
        author = try c.decodeIfPresent(String.self, forKey: .author)
        avatar = try c.decodeIfPresent(String.self, forKey: .avatar)
        body = try c.decodeIfPresent(String.self, forKey: .body) ?? ""
        at = try c.decodeIfPresent(String.self, forKey: .at) ?? ""
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? ""
        resolved = try c.decodeIfPresent(Bool.self, forKey: .resolved) ?? false
        threads = try c.decodeIfPresent([PRThread].self, forKey: .threads) ?? []
    }
    enum K: String, CodingKey { case id, kind, author, avatar, body, at, state, resolved, threads }
}

// One review-comment thread: root comment + replies, with its resolved state.
struct PRThread: Decodable, Identifiable, Equatable {
    var resolved = false
    var path: String? = nil
    var line: Int? = nil
    var at: String = ""
    var comments: [PRReviewComment] = []
    var id: String { (path ?? "") + String(line ?? 0) + String((comments.first?.body ?? "").prefix(16)) }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        resolved = try c.decodeIfPresent(Bool.self, forKey: .resolved) ?? false
        path = try c.decodeIfPresent(String.self, forKey: .path)
        line = try c.decodeIfPresent(Int.self, forKey: .line)
        at = try c.decodeIfPresent(String.self, forKey: .at) ?? ""
        comments = try c.decodeIfPresent([PRReviewComment].self, forKey: .comments) ?? []
    }
    enum K: String, CodingKey { case resolved, path, line, at, comments }
}

// GitHub App / bot avatars are served from the installation path (/in/), users from
// /u/ — so the avatar URL alone tells us whether to show the "Bot" tag.
func prIsBot(_ avatar: String?) -> Bool { (avatar ?? "").contains("/in/") }

// GitHub timestamps are ISO8601 (usually no fractional seconds). Render as a short
// relative age ("3d ago") for the conversation timeline.
func prRelDate(_ iso: String) -> String {
    guard !iso.isEmpty else { return "" }
    let withFrac = ISO8601DateFormatter()
    withFrac.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let plain = ISO8601DateFormatter()
    plain.formatOptions = [.withInternetDateTime]
    guard let d = withFrac.date(from: iso) ?? plain.date(from: iso) else { return "" }
    let s = Int(Date().timeIntervalSince(d))
    if s < 60 { return "just now" }
    if s < 3600 { return "\(s / 60)m ago" }
    if s < 86400 { return "\(s / 3600)h ago" }
    if s < 604800 { return "\(s / 86400)d ago" }
    return "\(s / 604800)w ago"
}
