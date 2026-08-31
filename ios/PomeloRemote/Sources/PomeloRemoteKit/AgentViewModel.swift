import Foundation

@MainActor
final class AgentViewModel: ObservableObject {
    @Published private(set) var output = ""
    @Published private(set) var streaming = false
    @Published var draft = ""
    @Published var lastError = ""

    let client: RemoteClient
    let branch: String
    let isMain: Bool
    private var task: Task<Void, Never>?

    init(client: RemoteClient, branch: String, isMain: Bool) {
        self.client = client; self.branch = branch; self.isMain = isMain
    }

    func start() {
        guard task == nil else { return }
        streaming = true
        task = Task {
            do {
                for try await frame in client.agentStream(branch: branch, isMain: isMain) {
                    if let f = PomJSON.decode(AgentFrame.self, from: frame), !f.text.isEmpty {
                        output += f.text
                    }
                }
            } catch {
                lastError = describe(error)
            }
            streaming = false
        }
    }

    func stop() {
        task?.cancel()
        task = nil
        streaming = false
    }

    func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        draft = ""
        do {
            try await client.nudge(branch: branch, isMain: isMain, text: text)
        } catch {
            lastError = describe(error)
        }
    }
}
