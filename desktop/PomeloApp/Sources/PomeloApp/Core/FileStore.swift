import Foundation

enum FileStore {
    nonisolated static func list(branch: String, isMain: Bool) -> Data {
        PomCore.shared.workspaceFilesData(branch: branch, isMain: isMain)
    }
    nonisolated static func read(branch: String, repo: String, path: String, isMain: Bool) -> Data {
        PomCore.shared.fileReadData(branch: branch, repo: repo, path: path, isMain: isMain)
    }
    nonisolated static func gitStatus(branch: String, isMain: Bool) -> Data {
        PomCore.shared.gitStatusData(branch: branch, isMain: isMain)
    }
    nonisolated static func gitDiff(branch: String, repo: String, path: String, isMain: Bool) -> Data {
        PomCore.shared.gitFileDiffData(branch: branch, repo: repo, path: path, isMain: isMain)
    }
    nonisolated static func gitBlame(branch: String, repo: String, path: String, isMain: Bool) -> Data {
        PomCore.shared.gitFileBlameData(branch: branch, repo: repo, path: path, isMain: isMain)
    }
}
