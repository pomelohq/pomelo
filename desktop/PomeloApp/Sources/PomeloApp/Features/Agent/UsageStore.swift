import SwiftUI

// `ClaudeUsage` now lives in the shared PomeloCore package (re-exported via
// Core/PomCore+DataSource.swift), decoded identically by the iOS app.

// One owner of Claude usage, shared across windows. Polls the core off the main
// thread and only republishes on an actual change (delta-guard), so observers are
// not invalidated every tick. A future core-side stream replaces the poll without
// touching the View. See ADR 0001 (subscribe / target shape).
@MainActor
final class UsageStore: ObservableObject {
    static let shared = UsageStore()
    @Published private(set) var usage: ClaudeUsage?

    private var started = false

    func start() {
        guard !started else { return }
        started = true
        Task { await loop() }
    }

    private func loop() async {
        while !Task.isCancelled {
            let d = await Task.detached(priority: .utility) { PomCore.shared.claudeUsageData() }.value
            if let next = PomJSON.decode(ClaudeUsage.self, from: d) {
                if next != usage { usage = next }
                // Mirror into the core so the remote (phone) serves this cached value
                // instead of calling the rate-limited upstream itself.
                if next.ok == true {
                    await Task.detached(priority: .utility) {
                        _ = PomCore.shared.command(domain: "claude", action: "usage_set", params: d)
                    }.value
                }
            }
            try? await Task.sleep(nanoseconds: 60_000_000_000)
        }
    }
}
