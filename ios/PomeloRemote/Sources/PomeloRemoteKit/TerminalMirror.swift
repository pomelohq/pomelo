import SwiftUI
import UIKit
import QuartzCore
import PomeloTerminalKit

// The GPU terminal host (MetalTerminalHostView) lives in the shared PomeloTerminalKit
// package; here we wrap it in a container + freeze overlay and drive it from the SSE
// PTY stream. All keyboard/scroll input goes to the remote PTY via the controller.

final class TerminalContainer: UIView {
    let terminal: MetalTerminalHostView
    private var freeze: UIView?

    init(terminal: MetalTerminalHostView) {
        self.terminal = terminal
        super.init(frame: .zero)
        addSubview(terminal)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layoutSubviews() {
        super.layoutSubviews()
        terminal.frame = bounds
        freeze?.frame = bounds
    }

    // Overlay a snapshot of the current terminal so the reconnect's scrollback replay
    // repaints behind it instead of flashing a cleared screen. snapshotView captures the
    // GPU-rendered content (layer.render(in:) does not for a CAMetalLayer).
    func showFreeze() {
        guard bounds.width > 0, bounds.height > 0 else { return }
        freeze?.removeFromSuperview()
        guard let snap = terminal.snapshotView(afterScreenUpdates: false) else { return }
        snap.frame = bounds
        addSubview(snap)
        freeze = snap
    }

    func hideFreeze() {
        guard let f = freeze else { return }
        UIView.animate(withDuration: 0.12, animations: { f.alpha = 0 }) { _ in
            f.removeFromSuperview()
            if self.freeze === f { self.freeze = nil }
        }
    }
}

@MainActor
final class TerminalController: ObservableObject {
    weak var view: MetalTerminalHostView?
    weak var container: TerminalContainer?
    private var client: RemoteClient?
    private(set) var window = ""
    private var task: Task<Void, Never>?
    private var didAttachOnce = false
    @Published var ended = false

    func attach(_ v: MetalTerminalHostView, container: TerminalContainer, client: RemoteClient, window: String) {
        self.view = v
        self.container = container
        self.client = client
        self.window = window
        start()
    }

    private var userClosed = false
    private var failures = 0
    private var offset: UInt64 = 0
    private var bgTask: UIBackgroundTaskIdentifier = .invalid

    // Hold a background assertion so a brief app switch doesn't suspend the app and
    // tear down the SSE stream; a dropped stream forces a snapshot replay that flickers.
    private func beginBG() {
        guard bgTask == .invalid else { return }
        bgTask = UIApplication.shared.beginBackgroundTask(withName: "pty") { [weak self] in
            self?.endBG()
        }
    }

    private func endBG() {
        guard bgTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(bgTask)
        bgTask = .invalid
    }

    func start() {
        guard task == nil, let client, !window.isEmpty else { return }
        ended = false
        userClosed = false
        beginBG()
        let reconnect = didAttachOnce
        didAttachOnce = true
        if reconnect { container?.showFreeze() }
        let startedAt = Date()
        task = Task { [weak self] in
            let since = self?.offset ?? 0
            do {
                for try await frame in client.ptyStream(window: self?.window ?? "", cols: 0, rows: 0, since: since) {
                    guard let self else { break }
                    switch frame {
                    case .reset:
                        // Server sends this before a scrollback replay so the buffer
                        // rebuilds clean instead of interleaving with the stale screen.
                        self.view?.feed(Array("\u{1b}c".utf8))
                    case .output(let bytes):
                        self.view?.feed(bytes)
                        self.offset += UInt64(bytes.count)
                    case .synced(let seq):
                        self.offset = seq
                        let c = self.container
                        Task { @MainActor in
                            try? await Task.sleep(nanoseconds: 120_000_000)
                            c?.hideFreeze()
                        }
                    case .control:
                        break
                    }
                }
            } catch {}
            guard let self else { return }
            self.task = nil
            if Task.isCancelled || self.userClosed { return }
            self.failures = Date().timeIntervalSince(startedAt) > 1.5 ? 0 : self.failures + 1
            if self.failures >= 4 { self.ended = true; self.endBG(); return }
            Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: 700_000_000)
                guard let self, self.task == nil, !self.userClosed else { return }
                self.start()
            }
        }
    }

    func resumeIfDropped() {
        guard task == nil, !userClosed, !ended else { return }
        failures = 0
        start()
    }

    func close() {
        guard let client, !window.isEmpty else { return }
        let w = window
        userClosed = true
        Task { await client.closeAgent(window: w) }
        task?.cancel(); task = nil
        ended = true
        endBG()
    }

    func send(_ text: String) {
        guard let client, !window.isEmpty else { return }
        Task { await client.ptyInput(window: window, data: Array((text + "\r").utf8)) }
    }

    func input(_ bytes: [UInt8]) {
        guard let client, !window.isEmpty else { return }
        Task { await client.ptyInput(window: window, data: bytes) }
    }

    func wheel(up: Bool, count: Int) {
        guard let client, !window.isEmpty, count > 0 else { return }
        let cb = up ? 64 : 65
        var bytes: [UInt8] = []
        for _ in 0..<min(count, 6) { bytes += Array("\u{1b}[<\(cb);1;1M".utf8) }
        Task { await client.ptyInput(window: window, data: bytes) }
    }

    func resize(cols: Int, rows: Int) {
        guard let client, !window.isEmpty, cols > 0, rows > 0 else { return }
        Task { await client.ptyResize(window: window, cols: cols, rows: rows) }
    }

    private var didSync = false

    func handleSize(cols: Int, rows: Int) {
        guard cols > 0, rows > 0 else { return }
        resize(cols: cols, rows: rows)
        guard !didSync else { return }
        didSync = true
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 600_000_000)
            resize(cols: cols, rows: max(1, rows - 1))
            try? await Task.sleep(nanoseconds: 150_000_000)
            resize(cols: cols, rows: rows)
        }
    }

    func stop() { task?.cancel(); task = nil; didSync = false; endBG() }
}

struct PtyTerminalView: UIViewRepresentable {
    let client: RemoteClient
    let window: String
    @ObservedObject var ctl: TerminalController
    var fontSize: CGFloat = 11
    var fontFamily: String = ""

    func makeUIView(context: Context) -> TerminalContainer {
        let tv = MetalTerminalHostView(frame: .zero)
        tv.setFont(family: fontFamily, size: fontSize)
        tv.applyColors(fg: Self.simd4(Theme.fg), bg: Self.simd4(Theme.bg))
        tv.onResize = { cols, rows in MainActor.assumeIsolated { ctl.handleSize(cols: cols, rows: rows) } }
        let container = TerminalContainer(terminal: tv)
        ctl.attach(tv, container: container, client: client, window: window)
        return container
    }

    func updateUIView(_ uiView: TerminalContainer, context: Context) {
        uiView.terminal.setFont(family: fontFamily, size: fontSize)
        uiView.terminal.applyColors(fg: Self.simd4(Theme.fg), bg: Self.simd4(Theme.bg))
    }

    private static func simd4(_ color: Color) -> SIMD4<Float> {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        UIColor(color).getRed(&r, green: &g, blue: &b, alpha: &a)
        return SIMD4(Float(r), Float(g), Float(b), 1)
    }
}
