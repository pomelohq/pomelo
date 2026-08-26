import Foundation

@MainActor
final class NMStoreViewModel: ObservableObject {
    struct Consumer: Decodable, Hashable { var branch = ""; var is_main = false }
    struct Entry: Decodable, Identifiable {
        var repo = ""; var hash = ""; var bytes: Int64 = 0; var current = false
        var consumers: [Consumer] = []
        var id: String { repo + "/" + hash }
        var orphan: Bool { consumers.isEmpty }
    }
    struct Unopt: Decodable, Identifiable { var branch = ""; var is_main = false; var repo = ""; var hash = ""; var id: String { branch + "/" + repo + "/" + hash } }
    struct Payload: Decodable { var entries: [Entry] = []; var total: Int64 = 0; var unoptimized: [Unopt] = [] }

    struct ReconcileResult: Decodable { var added = 0; var bytes: Int64 = 0 }
    struct ReclaimResult: Decodable { var relinked = 0; var reclaimed: Int64 = 0 }

    @Published private(set) var entries: [Entry] = []
    @Published private(set) var unoptimized: [Unopt] = []
    @Published private(set) var total: Int64 = 0
    @Published private(set) var loading = true
    @Published var optimizing = false
    @Published var lastOptimize: String?

    private let api: CoreAPI
    init(api: CoreAPI = PomCore.shared) { self.api = api }

    var stale: [Entry] { entries.filter { $0.orphan } }
    var staleBytes: Int64 { stale.reduce(0) { $0 + $1.bytes } }
    var sorted: [Entry] {
        entries.sorted { ($0.current ? 1 : 0, $0.bytes) > ($1.current ? 1 : 0, $1.bytes) }
    }

    func human(_ b: Int64) -> String {
        let g = Double(b) / 1_073_741_824, m = Double(b) / 1_048_576
        if g >= 1 { return String(format: "%.2f GB", g) }
        return String(format: "%.0f MB", m)
    }

    func load() async {
        loading = true
        let d = await api.call { $0.nmStoreListData() }
        if let p = PomJSON.decode(Payload.self, from: d) { entries = p.entries; total = p.total; unoptimized = p.unoptimized }
        loading = false
    }

    func delete(_ e: Entry) async {
        _ = await api.call { $0.nmStoreDelete(repo: e.repo, hash: e.hash) }
        await load()
    }

    func deleteStale() async {
        for e in stale { _ = await api.call { $0.nmStoreDelete(repo: e.repo, hash: e.hash) } }
        await load()
    }

    func optimize() async {
        optimizing = true
        let d = await api.call { $0.nmStoreReconcile() }
        let r = PomJSON.decode(ReconcileResult.self, from: d)
        await load()
        optimizing = false
        if let r, r.added > 0 {
            lastOptimize = "Cached \(r.added) new (\(human(r.bytes)))"
        } else {
            lastOptimize = "Already optimized"
        }
    }

    func reclaim() async {
        optimizing = true
        let d = await api.call { $0.nmStoreReclaim() }
        let r = PomJSON.decode(ReclaimResult.self, from: d)
        await load()
        optimizing = false
        if let r {
            lastOptimize = r.reclaimed > 0 ? "Reclaimed \(human(r.reclaimed)) (\(r.relinked) relinked)" : "Nothing to reclaim"
        }
    }
}
