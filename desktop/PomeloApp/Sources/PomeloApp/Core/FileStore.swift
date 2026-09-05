import Foundation

enum FileStore {
    nonisolated static func list(branch: String, isMain: Bool) -> Data {
        PomCore.shared.workspaceFilesData(branch: branch, isMain: isMain)
    }
    nonisolated static func read(branch: String, repo: String, path: String, isMain: Bool) -> Data {
        PomCore.shared.fileReadData(branch: branch, repo: repo, path: path, isMain: isMain)
    }
}
