import Foundation

@MainActor
final class AgentsViewModel: ObservableObject {
    @Published var states: [String: String] = [:]

    private let api: WorkspaceAPI
    init(api: WorkspaceAPI = PomCore.shared) { self.api = api }

    struct Note: Decodable, Sendable { let ws: String; let title: String; let event: String }

    func refresh(notify: Bool, whenFocused: Bool, activeSelection: String?, appActive: Bool, onNote: (_ title: String, _ event: String, _ wsKey: String) -> Void) async {
        let prev = states
        let result = await Task.detached(priority: .utility) { [api] () -> (states: [String: String], notes: [Note])? in
            struct R: Decodable { let states: [String: String] }
            guard let fresh = PomJSON.decode(R.self, from: api.query(domain: "agent_states", params: Data("{}".utf8)))?.states else { return nil }
            // The core classifies which transitions warrant a notification (ADR 0001).
            let params = (try? JSONSerialization.data(withJSONObject: ["prev": prev, "next": fresh])) ?? Data("{}".utf8)
            struct NR: Decodable { let notes: [Note] }
            let notes = PomJSON.decode(NR.self, from: api.query(domain: "agent_notifications", params: params))?.notes ?? []
            return (fresh, notes)
        }.value
        guard let result, result.states != states else { return }
        if notify {
            for n in result.notes {
                if appActive && n.ws == activeSelection && !whenFocused { continue }
                onNote(n.title, n.event, n.ws)
            }
        }
        states = result.states
    }
}
