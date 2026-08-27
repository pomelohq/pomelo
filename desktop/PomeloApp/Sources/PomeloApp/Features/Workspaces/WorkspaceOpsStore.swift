import Foundation

// FFI seam for workspace operations invoked from views (ADR 0001).
enum WorkspaceOpsStore {
    static func rename(branch: String, isMain: Bool, displayName: String) async {
        _ = await Task.detached(priority: .utility) {
            PomCore.shared.workspaceRename(branch: branch, isMain: isMain, displayName: displayName)
        }.value
    }

    static func mainPull(branch: String) async -> Data {
        await Task.detached(priority: .userInitiated) { PomCore.shared.mainPull(branch: branch) }.value
    }
}
