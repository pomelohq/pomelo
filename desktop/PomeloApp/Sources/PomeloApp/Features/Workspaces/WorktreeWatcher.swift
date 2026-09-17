import Foundation

// Watches a workspace's worktree tree with FSEvents (like Zed watches the project fs) and fires a debounced callback
// when files or the git index change, so the Git panel reflects working-tree changes in near real time instead of
// polling on a fixed interval.
final class WorktreeWatcher {
    private var stream: FSEventStreamRef?
    private var onChange: (() -> Void)?
    private let queue = DispatchQueue(label: "app.pomelo.worktree-watch")
    private var pending: DispatchWorkItem?

    func start(paths: [String], onChange: @escaping () -> Void) {
        stop()
        let roots = paths.filter { FileManager.default.fileExists(atPath: $0) }
        guard !roots.isEmpty else { return }
        self.onChange = onChange

        var ctx = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(),
                                       retain: nil, release: nil, copyDescription: nil)
        let callback: FSEventStreamCallback = { _, info, _, _, _, _ in
            guard let info else { return }
            Unmanaged<WorktreeWatcher>.fromOpaque(info).takeUnretainedValue().fire()
        }
        let flags = FSEventStreamCreateFlags(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer)
        guard let s = FSEventStreamCreate(kCFAllocatorDefault, callback, &ctx, roots as CFArray,
                                          FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 0.4, flags) else { return }
        FSEventStreamSetDispatchQueue(s, queue)
        FSEventStreamStart(s)
        stream = s
    }

    private func fire() {
        let work = DispatchWorkItem { [weak self] in self?.onChange?() }
        DispatchQueue.main.async { [weak self] in
            self?.pending?.cancel()
            self?.pending = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: work)
        }
    }

    func stop() {
        pending?.cancel(); pending = nil
        guard let s = stream else { return }
        FSEventStreamStop(s); FSEventStreamInvalidate(s); FSEventStreamRelease(s)
        stream = nil
    }

    deinit { stop() }
}
