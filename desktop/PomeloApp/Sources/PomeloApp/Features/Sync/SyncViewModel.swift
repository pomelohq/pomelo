import Foundation

@MainActor
final class SyncViewModel: ObservableObject {
    struct Payload: Decodable { var refresh_main = false; var refresh_interval_sec = 1800; var next_run_at: Int64 = 0 }

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
