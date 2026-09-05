import Foundation

// WsAgent / Workspace / WorkspacesResponse now come from the shared PomeloCore package
// (re-exported via RemoteClient.swift). Only iOS-specific models remain here.

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

// PomJSON now comes from the shared PomeloCore package (re-exported via RemoteClient.swift).
