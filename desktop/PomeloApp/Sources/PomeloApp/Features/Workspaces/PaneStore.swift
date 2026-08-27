import Foundation

// FFI seam for PTY pane views (ADR 0001). Threading matches the call sites:
// kill is fire-and-forget on a detached task; busy is a synchronous read.
enum PaneStore {
    nonisolated static func kill(paneID: String) {
        Task.detached(priority: .userInitiated) { _ = PomCore.shared.paneKill(paneID: paneID) }
    }
    nonisolated static func busy(holder: String) -> Data {
        PomCore.shared.paneBusyData(holder: holder)
    }
}
