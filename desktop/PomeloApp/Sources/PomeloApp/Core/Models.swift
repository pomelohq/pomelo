import Foundation
// Workspace / Repo / Service / Shortcut / WorkspacesResponse now live in the shared
// PomeloCore package (re-exported via Core/PomCore+DataSource.swift). Sessions stay here.

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
