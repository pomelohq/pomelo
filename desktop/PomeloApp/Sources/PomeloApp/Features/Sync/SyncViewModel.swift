import Foundation

@MainActor
final class SyncViewModel: ObservableObject {
    // Custom init so a missing key falls back to its default — synthesized Decodable
    // treats a defaulted-but-non-optional property as required (a payload without
    // next_run_at would otherwise throw and drop the whole decode).
    struct Payload: Decodable {
        var refresh_main = false
        var refresh_interval_sec = 1800
        var next_run_at: Int64 = 0
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            refresh_main = try c.decodeIfPresent(Bool.self, forKey: .refresh_main) ?? false
            refresh_interval_sec = try c.decodeIfPresent(Int.self, forKey: .refresh_interval_sec) ?? 1800
            next_run_at = try c.decodeIfPresent(Int64.self, forKey: .next_run_at) ?? 0
        }
        enum K: String, CodingKey { case refresh_main, refresh_interval_sec, next_run_at }
    }

    @Published var refreshMain = false
    @Published var intervalMin = 30
    @Published private(set) var nextRunAt: Date?
    @Published private(set) var loaded = false

    private let api: CoreAPI
    init(api: CoreAPI = PomCore.shared) { self.api = api }

    func load() async {
        let d = await api.call { $0.syncGetData() }
        if let p = PomJSON.decode(Payload.self, from: d) {
            refreshMain = p.refresh_main
            intervalMin = max(1, p.refresh_interval_sec / 60)
            nextRunAt = p.next_run_at > 0 ? Date(timeIntervalSince1970: TimeInterval(p.next_run_at)) : nil
        }
        loaded = true
    }

    var intervalSec: Int { intervalMin * 60 }

    func save() async {
        let on = refreshMain, sec = intervalSec
        _ = await api.call { $0.syncSet(refreshMain: on, intervalSec: sec) }
    }
}
