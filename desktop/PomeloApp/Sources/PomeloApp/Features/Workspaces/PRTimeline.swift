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
    var inline: [PRReviewComment] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decode(String.self, forKey: .id)
        kind = try c.decode(String.self, forKey: .kind)
        author = try c.decodeIfPresent(String.self, forKey: .author)
        avatar = try c.decodeIfPresent(String.self, forKey: .avatar)
        body = try c.decodeIfPresent(String.self, forKey: .body) ?? ""
        at = try c.decodeIfPresent(String.self, forKey: .at) ?? ""
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? ""
        inline = try c.decodeIfPresent([PRReviewComment].self, forKey: .inline) ?? []
    }
    enum K: String, CodingKey { case id, kind, author, avatar, body, at, state, inline }
}
