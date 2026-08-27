import Foundation

// FFI seam for the services board (ADR 0001). Threading mirrors each call site:
// fire-and-forget actions detach; reads called inside existing detached tasks stay
// synchronous; awaited actions get an off-main wrapper.
enum ServicesStore {
    private static func off(_ body: @escaping () -> Data) async -> Data {
        await Task.detached(priority: .userInitiated, operation: body).value
    }

    nonisolated static func openEditor(branch: String, isMain: Bool, repo: String, editor: String) {
        Task.detached(priority: .userInitiated) { _ = PomCore.shared.openEditor(branch: branch, isMain: isMain, repo: repo, editor: editor, resolveOnly: false) }
    }
    nonisolated static func serviceMode(repo: String, svc: String, mode: String) {
        Task.detached(priority: .userInitiated) { _ = PomCore.shared.serviceMode(repo: repo, svc: svc, mode: mode) }
    }
    nonisolated static func shortcutRun(branch: String, isMain: Bool, repo: String, cmd: String) -> Data {
        PomCore.shared.shortcutRun(branch: branch, isMain: isMain, repo: repo, cmd: cmd)
    }
    nonisolated static func serviceURL(branch: String, repo: String, svc: String) -> Data {
        PomCore.shared.serviceURL(branch: branch, repo: repo, svc: svc)
    }
    static func control(refJSON: String, action: String) async -> Data {
        await off { PomCore.shared.serviceControl(refJSON: refJSON, action: action) }
    }
    static func envSet(branch: String, isMain: Bool, repo: String, svc: String, env: String) async {
        _ = await off { PomCore.shared.envSet(branch: branch, isMain: isMain, repo: repo, svc: svc, env: env) }
    }
}
