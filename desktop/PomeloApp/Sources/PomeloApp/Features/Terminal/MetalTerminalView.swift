import AppKit
import SwiftUI
import SwiftTerm
import PomeloTerminalKit

// App-side glue for the shared GPU terminal (PomeloTerminalKit.MetalTerminalHostView).
// The renderer + all platform input handling live in the package; here we only wire the
// PTY stream (StreamManager) and map the app theme to the renderer's default colors.

// Namespace for the terminal's app-level constants/helpers (kept as `MetalTerminalView`
// so existing call sites — Settings font picker, stats toggle — are unchanged).
enum MetalTerminalView {
    static let statsKey = "metalTermStats"
    static func monospaceFamilies() -> [String] { TerminalFonts.monospaceFamilies() }

    static func themeColors(_ mode: ThemeMode) -> (fg: SIMD4<Float>, bg: SIMD4<Float>) {
        let pal: Palette = mode == .light ? Theme.light : (mode == .sepia ? Theme.sepia : Theme.dark)
        return (simd4(pal.fg), simd4(pal.bg))
    }

    private static func simd4(_ color: SwiftUI.Color) -> SIMD4<Float> {
        let ns = NSColor(color).usingColorSpace(.sRGB) ?? .black
        return SIMD4(Float(ns.redComponent), Float(ns.greenComponent), Float(ns.blueComponent), 1)
    }
}

// Spike harness: feeds a synthetic log firehose into the Metal renderer so the CPU
// cost can be compared against SwiftTerm under the same load. Open via the menu.
struct MetalTerminalSpike: NSViewRepresentable {
    func makeCoordinator() -> Coord { Coord() }
    func makeNSView(context: Context) -> MetalTerminalHostView {
        let v = MetalTerminalHostView(frame: .zero)
        context.coordinator.start(view: v)
        return v
    }
    func updateNSView(_ nsView: MetalTerminalHostView, context: Context) {}
    final class Coord {
        private var timer: Timer?
        private var n = 0
        func start(view: MetalTerminalHostView) {
            timer = Timer.scheduledTimer(withTimeInterval: 0.016, repeats: true) { [weak view] _ in
                guard let view else { return }
                var s = ""
                for _ in 0..<8 {
                    self.n += 1
                    let c = 31 + self.n % 200
                    s += "\u{1b}[38;5;\(c)m22:36:\(String(format: "%02d", self.n % 60)).\(String(format: "%03d", self.n % 1000)) I [98418:processor] {jobid:\(self.n)}\u{1b}[0m processed Worker in \(self.n % 500)ms\r\n"
                }
                view.feed(Array(s.utf8))
            }
        }
        deinit { timer?.invalidate() }
    }
}

struct OpenMetalSpikeButton: View {
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Button("Metal Terminal Spike") { openWindow(id: "metal-spike") }
            .keyboardShortcut("m", modifiers: [.command, .option, .control])
    }
}

struct MetalTerminalToggle: View {
    @AppStorage("metalTerminal") private var on = true
    var body: some View { Toggle("Use Metal Terminal (GPU)", isOn: $on) }
}

struct MetalTermStatsToggle: View {
    @AppStorage(MetalTerminalView.statsKey) private var on = false
    var body: some View { Toggle("Show Terminal Render Stats", isOn: $on) }
}

// Wires a real PTY holder stream to the shared Metal host (output + keyboard input).
// A drop-in for SwiftTerm's TerminalPane behind the experimental flag.
struct MetalTerminalPane: NSViewRepresentable {
    let holderName: String
    let wsKey: String
    var autorun: String? = nil
    var fontSize: CGFloat = 12
    var fontFamily: String = ""
    var themeMode: ThemeMode = activeThemeMode
    var onClosed: () -> Void = {}
    func makeCoordinator() -> Coord { Coord() }
    func makeNSView(context: Context) -> MetalTerminalHostView {
        let v = MetalTerminalHostView(frame: .zero)
        v.setFont(family: fontFamily, size: fontSize)
        let c = MetalTerminalView.themeColors(themeMode); v.applyColors(fg: c.fg, bg: c.bg)
        v.statsEnabled = UserDefaults.standard.bool(forKey: MetalTerminalView.statsKey)
        context.coordinator.attach(view: v, name: holderName, wsKey: wsKey, autorun: autorun, onClosed: onClosed)
        return v
    }
    func updateNSView(_ nsView: MetalTerminalHostView, context: Context) {
        nsView.setFont(family: fontFamily, size: fontSize)
        let c = MetalTerminalView.themeColors(themeMode); nsView.applyColors(fg: c.fg, bg: c.bg)
        nsView.statsEnabled = UserDefaults.standard.bool(forKey: MetalTerminalView.statsKey)
    }
    static func dismantleNSView(_ nsView: MetalTerminalHostView, coordinator: Coord) { coordinator.detach() }

    final class Coord {
        private var streamID: Int32 = 0
        private var closedFired = false
        @MainActor func attach(view: MetalTerminalHostView, name: String, wsKey: String, autorun: String?, onClosed: @escaping () -> Void) {
            view.onResize = { [weak self] cols, rows in
                guard let self, self.streamID > 0 else { return }
                StreamManager.shared.resize(self.streamID, cols: Int32(cols), rows: Int32(rows))
            }
            view.onInput = { [weak self] bytes in
                guard let self, self.streamID > 0 else { return }
                StreamManager.shared.send(self.streamID, bytes[...])
            }
            Task { @MainActor in
                let id = await StreamManager.shared.openPTY(name: name, wsKey: wsKey,
                                                            cols: Int32(view.termCols), rows: Int32(view.termRows)) { [weak self, weak view] kind, bytes in
                    if kind == .close {
                        if let self, !self.closedFired { self.closedFired = true; DispatchQueue.main.async { onClosed() } }
                        return
                    }
                    guard kind == .binary, let view else { return }
                    view.feed(bytes)
                }
                self.streamID = id
                if let cmd = autorun {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { StreamManager.shared.send(id, Array("\(cmd)\r".utf8)[...]) }
                }
            }
        }
        @MainActor func detach() { if streamID > 0 { StreamManager.shared.close(streamID); streamID = 0 } }
    }
}
