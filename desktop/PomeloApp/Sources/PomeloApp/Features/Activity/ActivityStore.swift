import Foundation

// FFI seam for the activity monitor (ADR 0001).
enum ActivityStore {
    private static func off(_ body: @escaping () -> Data) async -> Data {
        await Task.detached(priority: .utility, operation: body).value
    }
    static func serviceStop(refJSON: String) async { _ = await off { PomCore.shared.serviceControl(refJSON: refJSON, action: "stop") } }
    static func paneKill(paneID: String) async { _ = await off { PomCore.shared.paneKill(paneID: paneID) } }
    static func ps() async -> Data { await off { PomCore.shared.psData() } }
}
