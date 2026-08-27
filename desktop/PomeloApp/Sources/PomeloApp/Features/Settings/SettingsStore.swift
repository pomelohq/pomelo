import Foundation

// FFI seam for the settings panes (ADR 0001). Threading matches each call site:
// most reads are off-main; environments/configExplain stay synchronous.
enum SettingsStore {
    private static func off(_ body: @escaping () -> Data) async -> Data {
        await Task.detached(priority: .utility, operation: body).value
    }

    static func editors() async -> Data { await off { PomCore.shared.editorsData() } }
    static func version() async -> Data { await off { PomCore.shared.versionData() } }
    static func network() async -> Data { await off { PomCore.shared.networkData() } }
    static func setPorts(proxyPort: Int, webhookPort: Int) async { _ = await off { PomCore.shared.networkSetPorts(proxyPort: proxyPort, webhookPort: webhookPort) } }
    static func configFiles() async -> Data { await off { PomCore.shared.configFilesData() } }
    static func configFileGet(path: String) async -> Data { await off { PomCore.shared.configFileGetData(path: path) } }
    static func configFileCreate(name: String) async -> Data { await off { PomCore.shared.configFileCreate(name: name, yaml: "") } }
    static func configFileSet(path: String, yaml: String) async -> Data { await off { PomCore.shared.configFileSet(path: path, yaml: yaml, dry: false) } }
    static func configReload() async { _ = await off { PomCore.shared.configReload() } }

    nonisolated static func environments() -> Data { PomCore.shared.environmentsData() }
    nonisolated static func configExplain(repo: String, branch: String, svc: String, env: String) -> Data {
        PomCore.shared.configExplainData(repo: repo, branch: branch, svc: svc, env: env)
    }
}
