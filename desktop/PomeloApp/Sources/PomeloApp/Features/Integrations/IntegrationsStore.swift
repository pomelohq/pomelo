import Foundation

// FFI seam for the integrations settings pane (ADR 0001). Returns raw JSON; the
// view decodes into its local result shapes.
enum IntegrationsStore {
    private static func off(_ body: @escaping () -> Data) async -> Data {
        await Task.detached(priority: .utility, operation: body).value
    }

    static func setGithub(_ token: String) async { _ = await off { PomCore.shared.secretSet(name: "github", value: token) } }
    static func testGithub(_ token: String) async -> Data { await off { PomCore.shared.githubTest(token: token) } }
    static func status() async -> Data { await off { PomCore.shared.integrationsStatusData() } }
    static func setJira(site: String, email: String, token: String) async { _ = await off { PomCore.shared.jiraSet(site: site, email: email, token: token) } }
    static func testJira(site: String, email: String, token: String) async -> Data { await off { PomCore.shared.jiraTest(site: site, email: email, token: token) } }
}
