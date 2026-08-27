import Foundation

// FFI seam for the Claude terminal pane: resolve the host window and reap the pane.
// The View keeps its own UI state and calls these (ADR 0001).
@MainActor
final class ClaudeTerminalViewModel: ObservableObject {
    func resolveWindow(branch: String, isMain: Bool) async -> String? {
        await Task.detached(priority: .userInitiated) { () -> String? in
            struct R: Decodable { var window: String? }
            return PomJSON.decode(R.self, from: PomCore.shared.claudeTerminal(branch: branch, isMain: isMain))?.window
        }.value
    }

    nonisolated func kill(paneID: String) {
        Task.detached { _ = PomCore.shared.paneKill(paneID: paneID) }
    }
}
