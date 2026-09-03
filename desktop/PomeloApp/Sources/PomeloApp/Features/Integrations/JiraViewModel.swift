import SwiftUI

@MainActor
@Observable final class JiraViewModel {
    var issues: [String: JiraIssue] = [:]
    var configured = false

    private let api: JiraAPI
    init(api: JiraAPI = PomCore.shared) { self.api = api }

    func issueFor(_ branch: String) -> JiraIssue? { jiraKey(branch).flatMap { issues[$0] } }

    func refresh(branches: [String]) async {
        guard !branches.isEmpty else { return }
        struct R: Decodable { var configured = false; var issues: [String: JiraIssue]? }
        let r = await Task.detached(priority: .utility) { [api] in
            PomJSON.decode(R.self, from: api.jiraIssuesData(branches: branches))
        }.value
        guard let r else { return }
        if configured != r.configured { configured = r.configured }
        let iss = r.issues ?? [:]
        if iss != issues { withAnimation(.easeInOut(duration: 0.35)) { issues = iss } }
    }
}
