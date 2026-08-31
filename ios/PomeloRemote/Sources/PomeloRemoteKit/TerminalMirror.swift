import SwiftUI
import SwiftTerm
import UIKit

final class ReadOnlyTerminalView: TerminalView {
    override var canBecomeFirstResponder: Bool { false }
}

final class TerminalContainer: UIView {
    let terminal: ReadOnlyTerminalView
    private let freeze = UIImageView()

    init(terminal: ReadOnlyTerminalView) {
        self.terminal = terminal
        super.init(frame: .zero)
        addSubview(terminal)
        freeze.contentMode = .scaleToFill
        freeze.isHidden = true
        addSubview(freeze)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layoutSubviews() {
        super.layoutSubviews()
        terminal.frame = bounds
        freeze.frame = bounds
    }

    // Overlay a still of the current terminal so the reconnect's snapshot replay
    // repaints behind it instead of flashing a cleared screen.
    func showFreeze() {
        guard bounds.width > 0, bounds.height > 0 else { return }
        let r = UIGraphicsImageRenderer(bounds: bounds)
        freeze.image = r.image { ctx in terminal.layer.render(in: ctx.cgContext) }
        freeze.alpha = 1
        freeze.isHidden = false
    }

    func hideFreeze() {
        guard !freeze.isHidden else { return }
        UIView.animate(withDuration: 0.12, animations: { self.freeze.alpha = 0 }) { _ in
            self.freeze.isHidden = true
            self.freeze.image = nil
        }
    }
}

@MainActor
final class TerminalController: ObservableObject {
    weak var view: TerminalView?
    weak var container: TerminalContainer?
    private var client: RemoteClient?
    private(set) var window = ""
    private var task: Task<Void, Never>?
    private var didAttachOnce = false
    @Published var ended = false

    func attach(_ v: TerminalView, container: TerminalContainer, client: RemoteClient, window: String) {
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
                        self.view?.feed(byteArray: Array("\u{1b}c".utf8)[...])
                    case .output(let bytes):
                        self.view?.feed(byteArray: bytes[...])
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

    func makeUIView(context: Context) -> TerminalContainer {
        let tv = ReadOnlyTerminalView(frame: .zero)
        tv.terminalDelegate = context.coordinator
        tv.nativeBackgroundColor = UIColor(Theme.bg)
        tv.nativeForegroundColor = UIColor(Theme.fg)
        tv.backgroundColor = UIColor(Theme.bg)
        tv.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        tv.isUserInteractionEnabled = false
        let container = TerminalContainer(terminal: tv)
        ctl.attach(tv, container: container, client: client, window: window)
        return container
    }

    func updateUIView(_ uiView: TerminalContainer, context: Context) {
        let tv = uiView.terminal
        if abs(tv.font.pointSize - fontSize) > 0.1 {
            tv.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(ctl) }

    final class Coordinator: NSObject, TerminalViewDelegate {
        let ctl: TerminalController
        init(_ ctl: TerminalController) { self.ctl = ctl }

        func send(source: TerminalView, data: ArraySlice<UInt8>) {}
        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            MainActor.assumeIsolated { ctl.handleSize(cols: newCols, rows: newRows) }
        }
        func setTerminalTitle(source: TerminalView, title: String) {}
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
        func scrolled(source: TerminalView, position: Double) {}
        func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {}
        func bell(source: TerminalView) {}
        func clipboardCopy(source: TerminalView, content: Data) {}
        func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
    }
}
