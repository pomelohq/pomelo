import Foundation

@MainActor
@Observable final class WorkspacesViewModel {
    var workspaces: [Workspace] = [] {
        didSet { if oldValue.isEmpty && !workspaces.isEmpty { pushOrder() } } // first load = core is ready
    }
    var selection: String?
    var wsOrder: [String] = (UserDefaults.standard.array(forKey: "wsOrder") as? [String]) ?? [] {
        didSet { UserDefaults.standard.set(wsOrder, forKey: "wsOrder"); pushOrder() }
    }

    private let api: WorkspaceAPI
    init(api: WorkspaceAPI = PomCore.shared) { self.api = api; pushOrder() }

    // Mirror the drag-order into the core so the remote (phone) shows the same order.
    private func pushOrder() {
        let ids = wsOrder
        Task.detached {
            if let data = try? JSONSerialization.data(withJSONObject: ["order": ids]) {
                _ = PomCore.shared.command(domain: "ws", action: "order_set", params: data)
            }
        }
    }

    var mainWorkspaces: [Workspace] { WorkspacesVM.mainWorkspaces(workspaces) }
    var orderedNonMain: [Workspace] { WorkspacesVM.orderedNonMain(workspaces, order: wsOrder) }
    var allRepoNames: [String] { WorkspacesVM.allRepoNames(workspaces) }

    func moveWorkspace(from: IndexSet, to: Int) {
        var arr = orderedNonMain
        arr.move(fromOffsets: from, toOffset: to)
        wsOrder = arr.map(\.id)
    }

    func moveWorkspace(_ dragged: String, toIndex t: Int) {
        var arr = orderedNonMain.map(\.id)
        guard let from = arr.firstIndex(of: dragged) else { return }
        arr.remove(at: from)
        arr.insert(dragged, at: min(max(t, 0), arr.count))
        wsOrder = arr
    }

    func moveWorkspace(_ dragged: String, before target: String) {
        var arr = orderedNonMain.map(\.id)
        guard dragged != target, let from = arr.firstIndex(of: dragged) else { return }
        arr.remove(at: from)
        let to = arr.firstIndex(of: target) ?? arr.count
        arr.insert(dragged, at: to)
        wsOrder = arr
    }

    func fetch(git: Bool) async -> [Workspace]? {
        await Task.detached(priority: .utility) { [api] in
            PomJSON.decode(WorkspacesResponse.self, from: api.workspacesData(git: git))?.workspaces
        }.value
    }

    func fetchLiveness() async -> [Workspace]? {
        await Task.detached(priority: .utility) { [api] in
            PomJSON.decode(WorkspacesResponse.self, from: api.livenessData())?.workspaces
        }.value
    }

    func refreshLiveness() async {
        guard !workspaces.isEmpty, let live = await fetchLiveness() else { return }
        let structureChanged = WorkspacesVM.structureSignature(workspaces) != WorkspacesVM.structureSignature(live)
        let merged = WorkspacesVM.mergeLiveness(into: workspaces, from: live)
        if merged != workspaces { workspaces = merged }
        if structureChanged, let full = await fetch(git: true) {
            let refilled = WorkspacesVM.mergeLiveness(into: full, from: live)
            if refilled != workspaces { workspaces = refilled }
        }
    }
}

enum WorkspacesVM {
    static func mainWorkspaces(_ all: [Workspace]) -> [Workspace] {
        all.filter { $0.isMain }
    }

    static func orderedNonMain(_ all: [Workspace], order: [String]) -> [Workspace] {
        let rank = Dictionary(order.enumerated().map { ($1, $0) }, uniquingKeysWith: { a, _ in a })
        return all.filter { !$0.isMain }.sorted {
            (rank[$0.id] ?? Int.max, $0.branch) < (rank[$1.id] ?? Int.max, $1.branch)
        }
    }

    static func visibleNonMain(_ all: [Workspace], order: [String], hiding ops: Set<String>) -> [Workspace] {
        orderedNonMain(all, order: order).filter { !ops.contains($0.branch) }
    }

    static func allRepoNames(_ all: [Workspace]) -> [String] {
        Array(Set(all.flatMap { $0.repos.map(\.name) })).sorted()
    }

    static func structureSignature(_ all: [Workspace]) -> String {
        all.map { w in
            w.id + "[" + w.repos.map { r in
                r.name + ":" + (r.services ?? []).map(\.name).joined(separator: ",")
            }.joined(separator: "|") + "]"
        }.sorted().joined(separator: ";")
    }

    static func mergeLiveness(into base: [Workspace], from live: [Workspace]) -> [Workspace] {
        if !base.isEmpty && structureSignature(base) != structureSignature(live) { return live }
        let liveById = Dictionary(base.isEmpty ? [] : live.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        var out = base
        for i in out.indices {
            guard let l = liveById[out[i].id] else { continue }
            out[i].wsServices = l.wsServices
            out[i].running = l.running
            out[i].total = l.total
            let liveRepos = Dictionary(l.repos.map { ($0.name, $0) }, uniquingKeysWith: { a, _ in a })
            for j in out[i].repos.indices {
                guard let lr = liveRepos[out[i].repos[j].name] else { continue }
                let liveSvcs = Dictionary((lr.services ?? []).map { ($0.name, $0) }, uniquingKeysWith: { a, _ in a })
                var svcs = out[i].repos[j].services ?? []
                for k in svcs.indices {
                    guard let ls = liveSvcs[svcs[k].name] else { continue }
                    svcs[k].running = ls.running
                    svcs[k].crashed = ls.crashed
                    svcs[k].crashLog = ls.crashLog
                    svcs[k].port = ls.port
                    svcs[k].agentState = ls.agentState
                }
                out[i].repos[j].services = svcs
            }
        }
        return out
    }
}
