import Foundation

// FFI seam for the PR board (ADR 0001). All reads are synchronous, matching the
// call sites which already run inside their own detached tasks.
enum PRStore {
    nonisolated static func workspace(branch: String, isMain: Bool) -> Data { PomCore.shared.prWorkspaceData(branch: branch, isMain: isMain) }
    nonisolated static func localChanges(branch: String, isMain: Bool) -> Data { PomCore.shared.localChangesData(branch: branch, isMain: isMain) }
    nonisolated static func localDiff(branch: String, repo: String, isMain: Bool) -> Data { PomCore.shared.localDiffData(branch: branch, repo: repo, isMain: isMain) }
    nonisolated static func detail(branch: String, repo: String, isMain: Bool) -> Data { PomCore.shared.prDetailData(branch: branch, repo: repo, isMain: isMain) }
    nonisolated static func comments(branch: String, repo: String, isMain: Bool) -> Data { PomCore.shared.prCommentsData(branch: branch, repo: repo, isMain: isMain) }
    nonisolated static func timeline(branch: String, repo: String, isMain: Bool) -> Data { PomCore.shared.prTimelineData(branch: branch, repo: repo, isMain: isMain) }
    nonisolated static func refresh(branch: String, isMain: Bool) -> Data { PomCore.shared.prRefresh() }
    nonisolated static func commits(branch: String, repo: String, isMain: Bool) -> Data { PomCore.shared.prCommitsData(branch: branch, repo: repo, base: "", isMain: isMain) }
    nonisolated static func diff(branch: String, repo: String, isMain: Bool) -> Data { PomCore.shared.prDiffData(branch: branch, repo: repo, isMain: isMain) }
}
