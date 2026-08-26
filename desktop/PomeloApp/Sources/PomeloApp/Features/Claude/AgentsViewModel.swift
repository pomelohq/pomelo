import Foundation

@MainActor
final class AgentsViewModel: ObservableObject {
    @Published var states: [String: String] = [:]

    private let api: WorkspaceAPI
    init(api: WorkspaceAPI = PomCore.shared) { self.api = api }

    func refresh(notify: Bool, whenFocused: Bool, activeSelection: String?, appActive: Bool, onNote: (_ title: String, _ event: String, _ wsKey: String) -> Void) async {
        let fresh = await Task.detached(priority: .utility) { [api] in
            struct R: Decodable { let states: [String: String] }
            return PomJSON.decode(R.self, from: api.agentStatesData())?.states
        }.value
        guard let fresh, fresh != states else { return }
        if notify {
            for (ws, to) in fresh {
                let from = states[ws]
                if from == to { continue }
                if appActive && ws == activeSelection && !whenFocused { continue }
                if let m = Notifier.message(ws: ws, from: from, to: to) {
                    onNote(m.title, m.event, ws)
                }
            }
        }
        states = fresh
    }
}
