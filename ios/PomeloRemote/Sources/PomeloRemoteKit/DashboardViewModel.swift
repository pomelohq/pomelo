import Foundation

struct JiraLite: Hashable {
    var key = ""
    var summary = ""
    var status = ""
    var category = ""
}

@MainActor
final class DashboardViewModel: ObservableObject {
    @Published private(set) var workspaces: [WorkspaceRow] = []
    @Published private(set) var jira: [String: JiraLite] = [:]
    @Published private(set) var prCount: [String: Int] = [:]
    @Published private(set) var prSeverity: [String: String] = [:]
    @Published private(set) var agentStates: [String: String] = [:]
    @Published private(set) var reachable = true
    @Published var lastError = ""

    func agentState(_ ws: WorkspaceRow) -> String {
        agentStates[(ws.isMain ? "main:" : "ws:") + ws.branch] ?? ""
    }

    private var wsOrder: [String] = []

    let client: RemoteClient
    init(client: RemoteClient) { self.client = client }

    func load() async {
        do {
            if let od = try? await client.wsOrder(),
               let o = PomJSON.decode(WsOrderPayload.self, from: od) {
                wsOrder = o.order
            }
            let data = try await client.query("workspaces", ["git": false])
            if let p = PomJSON.decode(WorkspacesPayload.self, from: data) {
                let list = ordered(p.workspaces)
                if list != workspaces { workspaces = list }
            }
            if !reachable { reachable = true }
            if !lastError.isEmpty { lastError = "" }
            await loadAgentStates()
            await loadJira()
            await loadPRs()
            await loadUsage()
            publishSnapshot()
        } catch {
            reachable = false
            lastError = describe(error)
        }
    }

    private var sessionPct = 0
    private var weeklyPct = 0

    private func loadUsage() async {
        struct Win: Decodable {
            var pct: Double = 0
            init() {}
            enum K: String, CodingKey { case pct }
            init(from d: Decoder) throws { pct = (try? d.container(keyedBy: K.self).decode(Double.self, forKey: .pct)) ?? 0 }
        }
        struct U: Decodable {
            var session = Win(), weekly = Win()
            enum K: String, CodingKey { case session, weekly }
            init(from d: Decoder) throws {
                let c = try d.container(keyedBy: K.self)
                session = (try? c.decode(Win.self, forKey: .session)) ?? Win()
                weekly = (try? c.decode(Win.self, forKey: .weekly)) ?? Win()
            }
        }
        func norm(_ p: Double) -> Int { Int((min(1, max(0, p > 1 ? p / 100 : p))) * 100) }
        if let d = try? await client.claudeUsage(), let u = PomJSON.decode(U.self, from: d) {
            sessionPct = norm(u.session.pct)
            weeklyPct = norm(u.weekly.pct)
        }
    }

    private func publishSnapshot() {
        let rows = workspaces.map { ws -> AgentWorkspaceLite in
            AgentWorkspaceLite(
                branch: ws.branch, title: ws.title, isMain: ws.isMain,
                state: agentState(ws), running: ws.running, total: ws.total,
                prCount: prCount[ws.id] ?? 0)
        }
        let active = rows.filter { agentStateActive($0.state) }.count
        let mac = client.device.name.isEmpty ? client.device.host : client.device.name
        let snap = AgentSnapshot(mac: mac, updated: Date(), workspaces: rows, activeCount: active)
        LiveActivityController.shared.publish(mac: mac, snapshot: snap, sessionPct: sessionPct, weeklyPct: weeklyPct)
    }

    private func ordered(_ list: [WorkspaceRow]) -> [WorkspaceRow] {
        let rank = Dictionary(wsOrder.enumerated().map { ($1, $0) }, uniquingKeysWith: { a, _ in a })
        func key(_ w: WorkspaceRow) -> String { (w.isMain ? "main:" : "ws:") + w.branch }
        return list.sorted {
            (rank[key($0)] ?? Int.max, $0.branch) < (rank[key($1)] ?? Int.max, $1.branch)
        }
    }

    private struct WsOrderPayload: Decodable { var order: [String] = [] }

    private func loadAgentStates() async {
        struct P: Decodable { var states: [String: String] = [:] }
        if let d = try? await client.query("agent_states"), let p = PomJSON.decode(P.self, from: d) {
            if p.states != agentStates { agentStates = p.states }
        }
    }

    private func loadJira() async {
        guard let data = try? await client.jiraIssues(branches: workspaces.map(\.branch)),
              let p = PomJSON.decode(JiraPayload.self, from: data), p.configured else { return }
        var map: [String: JiraLite] = [:]
        for ws in workspaces {
            if let key = jiraKey(ws.branch), let iss = p.issues[key] {
                map[ws.branch] = JiraLite(key: iss.key, summary: iss.summary, status: iss.status, category: iss.category)
            }
        }
        if map != jira { jira = map }
    }

    private func loadPRs() async {
        guard let data = try? await client.prAll(),
              let groups = PomJSON.decode([String: PRGroup].self, from: data) else { return }
        var counts: [String: Int] = [:]
        var sev: [String: String] = [:]
        for ws in workspaces {
            let key = (ws.isMain ? "main:" : "ws:") + ws.branch
            counts[ws.id] = groups[key]?.prs.filter { $0.pr != nil }.count ?? 0
            sev[ws.id] = groups[key]?.severity ?? "ok"
        }
        if counts != prCount { prCount = counts }
        if sev != prSeverity { prSeverity = sev }
    }

    func pollLoop() async {
        while !Task.isCancelled {
            await load()
            try? await Task.sleep(nanoseconds: 5_000_000_000)
        }
    }

    private struct JiraPayload: Decodable {
        var configured = false
        var issues: [String: Issue] = [:]
        struct Issue: Decodable {
            var key = "", summary = "", status = "", category = ""
            init(from d: Decoder) throws {
                let c = try d.container(keyedBy: K.self)
                key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
                summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
                status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
                category = try c.decodeIfPresent(String.self, forKey: .category) ?? ""
            }
            enum K: String, CodingKey { case key, summary, status, category }
        }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            configured = try c.decodeIfPresent(Bool.self, forKey: .configured) ?? false
            issues = try c.decodeIfPresent([String: Issue].self, forKey: .issues) ?? [:]
        }
        enum K: String, CodingKey { case configured, issues }
    }

    private struct PRGroup: Decodable {
        var prs: [PRItem] = []
        var severity = "ok"
        struct PRItem: Decodable { var pr: PRRef? }
        struct PRRef: Decodable { var number: Int? }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            prs = try c.decodeIfPresent([PRItem].self, forKey: .prs) ?? []
            severity = try c.decodeIfPresent(String.self, forKey: .severity) ?? "ok"
        }
        enum K: String, CodingKey { case prs, severity }
    }
}

func jiraKey(_ branch: String) -> String? {
    let up = branch.uppercased()
    guard let re = try? NSRegularExpression(pattern: "[A-Z]+-[0-9]+"),
          let m = re.firstMatch(in: up, range: NSRange(up.startIndex..., in: up)),
          let r = Range(m.range, in: up) else { return nil }
    return String(up[r])
}

func describe(_ error: Error) -> String {
    if let e = error as? RemoteError {
        switch e {
        case .http(let c): return "HTTP \(c)"
        case .badURL: return "Bad address"
        case .notPaired: return "Not paired"
        }
    }
    return (error as NSError).localizedDescription
}
