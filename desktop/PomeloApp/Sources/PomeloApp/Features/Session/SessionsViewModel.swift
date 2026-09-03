import Foundation

@MainActor
@Observable final class SessionsViewModel {
    var sessions: [SessionItem] = []

    private let api: SessionAPI
    init(api: SessionAPI = PomCore.shared) { self.api = api }

    func load() async {
        let fresh = await Task.detached(priority: .utility) { [api] in
            PomJSON.decode(SessionsResponse.self, from: api.sessionListData())?.sessions
        }.value
        if let fresh, fresh != sessions { sessions = fresh }
    }
}
