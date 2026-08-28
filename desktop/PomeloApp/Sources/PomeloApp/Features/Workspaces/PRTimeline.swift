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
}
