import SwiftUI
import SwiftTerm

struct TerminalPane: NSViewRepresentable {
    let holderName: String
    let wsKey: String
    var autorun: String? = nil
    // When set, the pty stream is registered as this workspace's agent terminal so
    // "Ask agent" can type into it (see StreamManager.askClaude).
    var agentWsKey: String? = nil
    var fontSize: CGFloat = 12
    var themeMode: ThemeMode = activeThemeMode
    var onClosed: () -> Void = {}

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> TerminalView {
        let tv = TerminalView(frame: .init(x: 0, y: 0, width: 640, height: 400))
        tv.terminalDelegate = context.coordinator
        tv.font = .monospacedSystemFont(ofSize: fontSize, weight: .regular)
        tv.nativeBackgroundColor = NSColor(Theme.bg)
        tv.nativeForegroundColor = NSColor(Theme.fg)
        context.coordinator.onClosed = onClosed
        context.coordinator.attach(view: tv, name: holderName, wsKey: wsKey, autorun: autorun, agentWsKey: agentWsKey)
        return tv
    }

    func updateNSView(_ nsView: TerminalView, context: Context) {
        if abs(nsView.font.pointSize - fontSize) > 0.1 {
            nsView.font = .monospacedSystemFont(ofSize: fontSize, weight: .regular)
        }
        let bg = NSColor(Theme.bg), fg = NSColor(Theme.fg)
        if nsView.nativeBackgroundColor != bg { nsView.nativeBackgroundColor = bg }
        if nsView.nativeForegroundColor != fg { nsView.nativeForegroundColor = fg }
        context.coordinator.syncSize(nsView)
    }

    static func dismantleNSView(_ nsView: TerminalView, coordinator: Coordinator) {
        coordinator.detach()
    }

    final class Coordinator: NSObject, TerminalViewDelegate {
        private(set) var streamID: Int32 = 0
        private weak var view: TerminalView?
        private var pending: [UInt8] = []
        private var flushScheduled = false
        private var lastCols = 0, lastRows = 0
        private var sizeTimer: Timer?
        private var resizeWork: DispatchWorkItem?
        var onClosed: () -> Void = {}
        private var closedFired = false
        private var agentWsKey: String?

        @MainActor
        func syncSize(_ view: TerminalView) {
            let t = view.getTerminal()
            pushResize(cols: t.cols, rows: t.rows)
        }

        @MainActor
        func pushResize(cols: Int, rows: Int) {
            guard streamID > 0, cols > 0, rows > 0 else { return }
            resizeWork?.cancel()
            let id = streamID
            let w = DispatchWorkItem { [weak self] in
                guard let self, cols != self.lastCols || rows != self.lastRows else { return }
                self.lastCols = cols; self.lastRows = rows
                StreamManager.shared.resize(id, cols: Int32(cols), rows: Int32(rows))
            }
            resizeWork = w
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: w)
        }

        @MainActor
        func attach(view: TerminalView, name: String, wsKey: String, autorun: String? = nil, agentWsKey: String? = nil) {
            self.view = view
            self.agentWsKey = agentWsKey
            let size = view.getTerminal()
            let cols = Int32(size.cols), rows = Int32(size.rows)
            Task { @MainActor in
                let id = await StreamManager.shared.openPTY(name: name, wsKey: wsKey, cols: cols, rows: rows) { [weak self] kind, bytes in
                    guard let self else { return }
                    if kind == .close {
                        if !self.closedFired { self.closedFired = true; DispatchQueue.main.async { self.onClosed() } }
                        return
                    }
                    guard kind == .binary else { return }
                    self.pending.append(contentsOf: bytes)
                    if !self.flushScheduled {
                        self.flushScheduled = true
                        // Coalesce a firehose (log spam / fast agent output) to ~30fps so a
                        // SwiftTerm feed doesn't run on the main thread every runloop turn.
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.033) { self.flush() }
                    }
                }
                self.streamID = id
                if id > 0 { StreamManager.shared.registerTerminal(id, view: view) }
                if id > 0, let k = agentWsKey { StreamManager.shared.registerClaudeTerminal(id, wsKey: k) }
                if let cmd = autorun, id > 0 {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) {
                        StreamManager.shared.send(id, Array("\(cmd)\r".utf8)[...])
                    }
                }
            }
            sizeTimer?.invalidate()
            var stable = 0
            sizeTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] t in
                guard let self, let v = self.view else { t.invalidate(); return }
                MainActor.assumeIsolated {
                    let before = (self.lastCols, self.lastRows)
                    self.syncSize(v)
                    let term = v.getTerminal()
                    if term.cols == before.0 && term.rows == before.1 { stable += 1 } else { stable = 0 }
                    if stable >= 2 { t.invalidate() }
                }
            }
        }

        private func flush() {
            flushScheduled = false
            guard let view, !pending.isEmpty else { return }
            view.feed(byteArray: pending[...])
            pending.removeAll(keepingCapacity: true)
        }

        @MainActor
        func detach() {
            sizeTimer?.invalidate(); sizeTimer = nil
            if let k = agentWsKey { StreamManager.shared.unregisterClaudeTerminal(streamID, wsKey: k) }
            StreamManager.shared.close(streamID)
            streamID = 0
        }

        nonisolated func send(source: TerminalView, data: ArraySlice<UInt8>) {
            MainActor.assumeIsolated { StreamManager.shared.send(streamID, data) }
        }

        nonisolated func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            guard newCols > 0, newRows > 0 else { return }
            MainActor.assumeIsolated { pushResize(cols: newCols, rows: newRows) }
        }

        nonisolated func setTerminalTitle(source: TerminalView, title: String) {}
        nonisolated func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
        nonisolated func scrolled(source: TerminalView, position: Double) {}
        nonisolated func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
    }
}
