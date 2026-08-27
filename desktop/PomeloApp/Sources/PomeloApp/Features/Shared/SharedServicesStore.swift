import Foundation

// FFI seam for the shared-services views (ADR 0001). Returns raw JSON; views decode.
enum SharedServicesStore {
    private static func off(_ body: @escaping () -> Data) async -> Data {
        await Task.detached(priority: .utility, operation: body).value
    }

    static func status() async -> Data { await off { PomCore.shared.sharedStatusData() } }
    static func action(name: String, action: String) async { _ = await off { PomCore.shared.sharedAction(name: name, action: action) } }
    static func stack(action: String) async { _ = await off { PomCore.shared.sharedStack(action: action) } }
    static func stats(name: String) async -> Data { await off { PomCore.shared.sharedStatsData(name: name) } }
    static func inspect(name: String) async -> Data { await off { PomCore.shared.sharedInspectData(name: name) } }
    static func logs(name: String, lines: Int) async -> Data { await off { PomCore.shared.sharedLogsData(name: name, lines: lines) } }
}
