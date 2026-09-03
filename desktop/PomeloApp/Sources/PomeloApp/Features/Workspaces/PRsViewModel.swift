import SwiftUI

@MainActor
@Observable final class PRsViewModel {
    struct Group: Decodable, Equatable {
        var prs: [WorkspacePR] = []; var severity = "ok"
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            prs = try c.decodeIfPresent([WorkspacePR].self, forKey: .prs) ?? []
            severity = try c.decodeIfPresent(String.self, forKey: .severity) ?? "ok"
        }
        enum K: String, CodingKey { case prs, severity }
    }

    var wsPRs: [String: [WorkspacePR]] = [:]
    var wsSeverity: [String: String] = [:]
    var loading = true

    private let api: PRAPI
    init(api: PRAPI = PomCore.shared) { self.api = api }

    func prsFor(_ id: String) -> [WorkspacePR] { wsPRs[id] ?? [] }
    func severityFor(_ id: String) -> String { wsSeverity[id] ?? "ok" }

    @discardableResult
    func refresh() async -> Bool {
        let map = await Task.detached(priority: .utility) { [api] in
            PomJSON.decode([String: Group].self, from: api.prAllData())
        }.value
        if loading { loading = false }
        guard let map else { return false }
        let prs = map.mapValues(\.prs)
        let sev = map.mapValues(\.severity)
        guard prs != wsPRs || sev != wsSeverity else { return false }
        withAnimation(.easeInOut(duration: 0.35)) { wsPRs = prs; wsSeverity = sev }
        return true
    }
}
