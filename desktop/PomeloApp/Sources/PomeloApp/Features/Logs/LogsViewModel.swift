import Foundation

@MainActor
final class LogsViewModel: ObservableObject {
    struct Payload: Decodable {
        var version = ""; var session = ""; var logfile = ""; var lines: [String] = []
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            version = try c.decodeIfPresent(String.self, forKey: .version) ?? ""
            session = try c.decodeIfPresent(String.self, forKey: .session) ?? ""
            logfile = try c.decodeIfPresent(String.self, forKey: .logfile) ?? ""
            lines = try c.decodeIfPresent([String].self, forKey: .lines) ?? []
        }
        enum K: String, CodingKey { case version, session, logfile, lines }
    }

    @Published private(set) var lines: [String] = []
    @Published private(set) var version = ""
    @Published private(set) var session = ""
    @Published private(set) var logfile = ""
    @Published private(set) var loading = true

    private let api: CoreAPI
    init(api: CoreAPI = PomCore.shared) { self.api = api }

    var isEmpty: Bool { lines.isEmpty }

    func load() async {
        loading = true
        let d = await api.call { $0.logsData() }
        if let p = PomJSON.decode(Payload.self, from: d) {
            version = p.version; session = p.session; logfile = p.logfile; lines = p.lines
        }
        loading = false
    }

    func diagnostics(os: String, tailCount: Int = 400) -> String {
        let head = "Pomelo \(version)\n\(os)\nsession: \(session)\nlog: \(logfile)\n\n"
        return head + lines.suffix(tailCount).joined(separator: "\n")
    }
}
