import SwiftUI
import SwiftTerm

final class ReadOnlyTerminalView: TerminalView {
    override var canBecomeFirstResponder: Bool { false }
}

@MainActor
final class TerminalController: ObservableObject {
    weak var view: TerminalView?
    private var client: RemoteClient?
    private(set) var window = ""
    private var task: Task<Void, Never>?
    @Published var ended = false

    func attach(_ v: TerminalView, client: RemoteClient, window: String) {
        self.view = v
        self.client = client
        self.window = window
        start()
    }

    func start() {
        guard task == nil, let client, !window.isEmpty else { return }
        ended = false
        task = Task { [weak self] in
            do {
                for try await frame in client.ptyStream(window: self?.window ?? "", cols: 0, rows: 0) {
                    if case .output(let bytes) = frame { self?.view?.feed(byteArray: bytes[...]) }
                }
            } catch {}
            if !Task.isCancelled { self?.ended = true }
            self?.task = nil
        }
    }

    func close() {
        guard let client, !window.isEmpty else { return }
        let w = window
        Task { await client.closeAgent(window: w) }
        task?.cancel(); task = nil
        ended = true
    }

    func send(_ text: String) {
        guard let client, !window.isEmpty else { return }
        Task { await client.ptyInput(window: window, data: Array((text + "\r").utf8)) }
    }

    func input(_ bytes: [UInt8]) {
        guard let client, !window.isEmpty else { return }
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

    func stop() { task?.cancel(); task = nil; didSync = false }
}

struct PtyTerminalView: UIViewRepresentable {
    let client: RemoteClient
    let window: String
    @ObservedObject var ctl: TerminalController
    var fontSize: CGFloat = 11

    func makeUIView(context: Context) -> TerminalView {
        let tv = ReadOnlyTerminalView(frame: .zero)
        tv.terminalDelegate = context.coordinator
        tv.nativeBackgroundColor = UIColor(Theme.bg)
        tv.nativeForegroundColor = UIColor(Theme.fg)
        tv.backgroundColor = UIColor(Theme.bg)
        tv.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        tv.isUserInteractionEnabled = false
        ctl.attach(tv, client: client, window: window)
        return tv
    }

    func updateUIView(_ uiView: TerminalView, context: Context) {
        if abs(uiView.font.pointSize - fontSize) > 0.1 {
            uiView.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
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
