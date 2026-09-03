import SwiftUI

struct PeekAllResponse: Decodable { let windows: [String: [String]] }

@MainActor
final class PeekStore: ObservableObject {
    @Published var lines: [String: [String]] = [:]
    var isActive: () -> Bool = { true }
    private var task: Task<Void, Never>?
    private var currentKey = ""

    func sync(windows: [String]) {
        let key = windows.sorted().joined(separator: ",")
        guard key != currentKey else { return }
        currentKey = key
        task?.cancel()
        guard !windows.isEmpty else { lines = [:]; return }
        task = Task { [weak self] in
            while !Task.isCancelled {
                if self?.isActive() ?? false {
                    let wins = Array(windows)
                    let fresh = await Task.detached(priority: .utility) { () -> [String: [String]]? in
                        let d = PomCore.shared.peekAllData(windows: wins, lines: 8)
                        return PomJSON.decode(PeekAllResponse.self, from: d)?.windows
                    }.value
                    if Task.isCancelled { break }
                    if let fresh, fresh != self?.lines { PerfHUD.shared.tick("peek:pub"); self?.lines = fresh }
                }
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
        }
    }

    func stop() { task?.cancel(); task = nil; currentKey = "" }
}
