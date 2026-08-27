import Foundation

// FFI seam for config bundle export/import sheets (ADR 0001). Returns raw JSON;
// the sheets decode into their local result shapes.
enum BundleStore {
    private static func off(_ body: @escaping () -> Data) async -> Data {
        await Task.detached(priority: .utility, operation: body).value
    }

    static func secretNames() async -> Data { await off { PomCore.shared.secretNamesData() } }
    static func export(includeSecrets: Bool, password: String) async -> Data {
        await off { PomCore.shared.bundleExport(includeSecrets: includeSecrets, password: password) }
    }
    static func read(dataB64: String, password: String) async -> Data {
        await off { PomCore.shared.bundleRead(dataB64: dataB64, password: password) }
    }
    static func apply(dataB64: String, password: String, yaml: String, writeConfig: Bool, createSecrets: Bool) async -> Data {
        await off { PomCore.shared.bundleApply(dataB64: dataB64, password: password, yaml: yaml, writeConfig: writeConfig, createSecrets: createSecrets) }
    }
    static func adapt(dataB64: String, password: String, yaml: String, createSecrets: Bool) async -> Data {
        await off { PomCore.shared.bundleAdapt(dataB64: dataB64, password: password, yaml: yaml, createSecrets: createSecrets) }
    }
}
