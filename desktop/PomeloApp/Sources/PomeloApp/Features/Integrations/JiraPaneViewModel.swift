import Foundation

// Owns the Jira ticket detail for a workspace pane (ADR 0001).
@MainActor
final class JiraPaneViewModel: ObservableObject {
    @Published private(set) var detail: JiraDetail?
    @Published private(set) var loading = true
    @Published private(set) var reloading = false

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

    func reload(key: String?) async {
        guard let k = key, !reloading else { return }
        reloading = true
        let fresh = await Task.detached(priority: .userInitiated) { () -> JiraDetail? in
            PomJSON.decode(JiraDetail.self, from: PomCore.shared.jiraIssueData(key: k, force: true))
        }.value
        reloading = false
        if let fresh { detail = fresh }
    }
}
