import Foundation

@MainActor
final class NMStoreViewModel: ObservableObject {
    struct Consumer: Decodable, Hashable {
        var branch = ""; var is_main = false
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            branch = try c.decodeIfPresent(String.self, forKey: .branch) ?? ""
            is_main = try c.decodeIfPresent(Bool.self, forKey: .is_main) ?? false
        }
        enum K: String, CodingKey { case branch, is_main }
    }
    struct Entry: Decodable, Identifiable {
        var repo = ""; var hash = ""; var bytes: Int64 = 0; var current = false
        var orphan = false            // decided by the core (ADR 0001), not derived here
        var consumers: [Consumer] = []
        var id: String { repo + "/" + hash }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
            hash = try c.decodeIfPresent(String.self, forKey: .hash) ?? ""
            bytes = try c.decodeIfPresent(Int64.self, forKey: .bytes) ?? 0
            current = try c.decodeIfPresent(Bool.self, forKey: .current) ?? false
            orphan = try c.decodeIfPresent(Bool.self, forKey: .orphan) ?? false
            consumers = try c.decodeIfPresent([Consumer].self, forKey: .consumers) ?? []
        }
        enum K: String, CodingKey { case repo, hash, bytes, current, orphan, consumers }
    }
    struct Unopt: Decodable, Identifiable {
        var branch = ""; var is_main = false; var repo = ""; var hash = ""; var id: String { branch + "/" + repo + "/" + hash }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            branch = try c.decodeIfPresent(String.self, forKey: .branch) ?? ""
            is_main = try c.decodeIfPresent(Bool.self, forKey: .is_main) ?? false
            repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
            hash = try c.decodeIfPresent(String.self, forKey: .hash) ?? ""
        }
        enum K: String, CodingKey { case branch, is_main, repo, hash }
    }
    struct Payload: Decodable {
        var entries: [Entry] = []; var total: Int64 = 0; var unoptimized: [Unopt] = []
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            entries = try c.decodeIfPresent([Entry].self, forKey: .entries) ?? []
            total = try c.decodeIfPresent(Int64.self, forKey: .total) ?? 0
            unoptimized = try c.decodeIfPresent([Unopt].self, forKey: .unoptimized) ?? []
        }
        enum K: String, CodingKey { case entries, total, unoptimized }
    }

    @Published private(set) var entries: [Entry] = []
    @Published private(set) var unoptimized: [Unopt] = []
    @Published private(set) var total: Int64 = 0
    @Published private(set) var loading = true

    private let api: CoreAPI
    init(api: CoreAPI = PomCore.shared) { self.api = api }

    var stale: [Entry] { entries.filter { $0.orphan } }
    var staleBytes: Int64 { stale.reduce(0) { $0 + $1.bytes } }
    var sorted: [Entry] { entries }   // core already orders in-use-first, largest-first

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

}
