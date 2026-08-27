import Foundation

// FFI seam for session create/delete + onboarding install/doctor (ADR 0001).
enum SessionStore {
    private static func off(_ body: @escaping () -> Data) async -> Data {
        await Task.detached(priority: .utility, operation: body).value
    }
    static func create(json: String) async -> Data { await off { PomCore.shared.sessionCreate(json: json) } }
    static func delete(name: String, purge: Bool) async { _ = await off { PomCore.shared.sessionDelete(name: name, purge: purge) } }
    static func installDeps(branch: String, isMain: Bool) async -> Data { await off { PomCore.shared.installDeps(branch: branch, isMain: isMain) } }
    static func doctor() async -> Data { await off { PomCore.shared.doctorData() } }
}
