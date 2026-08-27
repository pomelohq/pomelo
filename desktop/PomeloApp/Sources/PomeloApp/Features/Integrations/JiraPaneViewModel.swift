import Foundation

// Owns the Jira ticket detail for a workspace pane (ADR 0001).
@MainActor
final class JiraPaneViewModel: ObservableObject {
    @Published private(set) var detail: JiraDetail?
    @Published private(set) var loading = true

    func load(key: String?) async {
        detail = nil
        loading = true
        guard let k = key else { loading = false; return }
        let fresh = await Task.detached(priority: .userInitiated) { () -> JiraDetail? in
            PomJSON.decode(JiraDetail.self, from: PomCore.shared.jiraIssueData(key: k))
        }.value
        loading = false
        detail = fresh
    }
}
