import AppKit
import SwiftUI
import CoreText
import QuartzCore
import SwiftTerm
import PomeloTerminalKit

// Spike harness: feeds a synthetic log firehose into the Metal renderer so the CPU
// cost can be compared against SwiftTerm under the same load. Open via the menu.
struct MetalTerminalSpike: NSViewRepresentable {
    func makeCoordinator() -> Coord { Coord() }
    func makeNSView(context: Context) -> MetalTerminalView {
        let v = MetalTerminalView(frame: .zero)
        context.coordinator.start(view: v)
        return v
    }
    func updateNSView(_ nsView: MetalTerminalView, context: Context) {}
    final class Coord {
        private var timer: Timer?
        private var n = 0
        func start(view: MetalTerminalView) {
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

// Wires a real PTY holder stream to the Metal renderer (output + keyboard input).
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
    func makeNSView(context: Context) -> MetalTerminalView {
        let v = MetalTerminalView(frame: .zero)
        v.setFont(family: fontFamily, size: fontSize)
        v.applyTheme(themeMode)
        context.coordinator.attach(view: v, name: holderName, wsKey: wsKey, autorun: autorun, onClosed: onClosed)
        return v
    }
    func updateNSView(_ nsView: MetalTerminalView, context: Context) { nsView.setFont(family: fontFamily, size: fontSize); nsView.applyTheme(themeMode) }
    static func dismantleNSView(_ nsView: MetalTerminalView, coordinator: Coord) { coordinator.detach() }

    final class Coord {
        private var streamID: Int32 = 0
        private var closedFired = false
        @MainActor func attach(view: MetalTerminalView, name: String, wsKey: String, autorun: String?, onClosed: @escaping () -> Void) {
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

// Thin AppKit host around the shared `TerminalRenderer` (PomeloTerminalKit). It forwards
// NSEvent input (keys, mouse, scroll, selection) to the renderer's semantic API and maps
// the app theme to the renderer's default colors. The GPU rendering lives in the package.
final class MetalTerminalView: NSView {
    static let statsKey = "metalTermStats"

    let renderer = TerminalRenderer()
    private let statsLabel = NSTextField(labelWithString: "")
    private var displayLink: CADisplayLink?

    var terminal: Terminal { renderer.terminal }
    var termCols: Int { renderer.termCols }
    var termRows: Int { renderer.termRows }
    var onResize: ((Int, Int) -> Void)?
    var onInput: ([UInt8]) -> Void = { _ in }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer = renderer.metalLayer
        renderer.onResize = { [weak self] c, r in self?.onResize?(c, r) }
        renderer.onStats = { [weak self] s in self?.statsLabel.stringValue = s }
        renderer.statsEnabled = UserDefaults.standard.bool(forKey: Self.statsKey)
        statsLabel.font = .monospacedSystemFont(ofSize: 11, weight: .semibold)
        statsLabel.textColor = .systemGreen
        statsLabel.backgroundColor = NSColor.black.withAlphaComponent(0.85)
        statsLabel.drawsBackground = true
        statsLabel.frame = NSRect(x: 8, y: 8, width: 620, height: 18)
        statsLabel.isHidden = !renderer.statsEnabled
        addSubview(statsLabel)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func feed(_ bytes: [UInt8]) { renderer.feed(bytes) }
    func feed(_ bytes: ArraySlice<UInt8>) { renderer.feed(bytes) }

    func setFont(family: String, size: CGFloat) {
        renderer.setFont(family: family, size: size)
        needsLayout = true
    }

    func applyTheme(_ mode: ThemeMode) {
        let c = Self.themeColors(mode)
        renderer.setColors(fg: c.fg, bg: c.bg)
    }

    static func monospaceFamilies() -> [String] { TerminalFonts.monospaceFamilies() }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil, displayLink == nil {
            let link = displayLink(target: renderer, selector: #selector(TerminalRenderer.renderTick))
            link.add(to: .main, forMode: .common)
            displayLink = link
        } else if window == nil {
            displayLink?.invalidate(); displayLink = nil
            renderer.teardown()
        }
    }

    override func layout() {
        super.layout()
        let scale = window?.backingScaleFactor ?? 2
        statsLabel.frame = NSRect(x: 8, y: bounds.height - 24, width: 620, height: 18)
        let show = UserDefaults.standard.bool(forKey: Self.statsKey)
        renderer.statsEnabled = show
        if statsLabel.isHidden != !show { statsLabel.isHidden = !show }
        renderer.updateGeometry(pointSize: bounds.size, scale: scale)
    }

    // MARK: - Focus

    override var acceptsFirstResponder: Bool { true }
    override func becomeFirstResponder() -> Bool { renderer.setFocused(true); return super.becomeFirstResponder() }
    override func resignFirstResponder() -> Bool { renderer.setFocused(false); return super.resignFirstResponder() }

    // MARK: - Mouse

    private enum DragKind { case none, select, app }
    private var dragKind: DragKind = .none

    private func cell(at event: NSEvent) -> (Int, Int) {
        let scale = window?.backingScaleFactor ?? 2
        let loc = convert(event.locationInWindow, from: nil)
        let cw = renderer.cellPixelWidth, ch = renderer.cellPixelHeight
        let col = min(max(0, Int(loc.x * scale / cw)), max(0, termCols - 1))
        let row = min(max(0, Int((bounds.height - loc.y) * scale / ch)), max(0, termRows - 1))
        return (col, row)
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        if renderer.mouseModeOn && !event.modifierFlags.contains(.option) {
            dragKind = .app; sendMouseSGR(event, code: 0, press: true)
            return
        }
        let (col, row) = cell(at: event)
        if event.clickCount == 2 { renderer.selectWord(col: col, screenRow: row); dragKind = .none; return }
        if event.clickCount >= 3 { renderer.selectLine(screenRow: row); dragKind = .none; return }
        dragKind = .select; renderer.beginSelect(col: col, screenRow: row)
    }

    override func mouseDragged(with event: NSEvent) {
        switch dragKind {
        case .app: sendMouseSGR(event, code: 32, press: true)
        case .select:
            let loc = convert(event.locationInWindow, from: nil)
            let scale = window?.backingScaleFactor ?? 2
            let col = min(max(0, Int(loc.x * scale / renderer.cellPixelWidth)), max(0, termCols - 1))
            if loc.y > bounds.height { renderer.beginAutoscroll(dir: -1, col: col) }
            else if loc.y < 0 { renderer.beginAutoscroll(dir: 1, col: col) }
            else { renderer.endAutoscroll(); let (c, r) = cell(at: event); renderer.extendSelect(col: c, screenRow: r) }
        case .none: break
        }
    }

    override func mouseUp(with event: NSEvent) {
        renderer.endAutoscroll()
        switch dragKind {
        case .app: sendMouseSGR(event, code: 0, press: false)
        case .select: renderer.endSelectClearIfEmpty()
        case .none: break
        }
        dragKind = .none
    }

    private func sendMouseSGR(_ event: NSEvent, code: Int, press: Bool) {
        let (c, r) = cell(at: event)
        let col = min(max(1, c + 1), max(1, termCols)), row = min(max(1, r + 1), max(1, termRows))
        onInput(Array("\u{1b}[<\(code);\(col);\(row)\(press ? "M" : "m")".utf8))
    }

    // MARK: - Scroll

    private var scrollAccum: CGFloat = 0

    override func scrollWheel(with event: NSEvent) {
        let wantsMouse = renderer.mouseModeOn
        let dy = event.scrollingDeltaY
        guard dy != 0 else { return }
        if (dy > 0) != (scrollAccum > 0) { scrollAccum = 0 }
        scrollAccum += dy

        if wantsMouse {
            let threshold: CGFloat = 3
            var ticks = 0
            while abs(scrollAccum) >= threshold, ticks < 24 {
                ticks += 1
                scrollAccum += scrollAccum > 0 ? -threshold : threshold
            }
            guard ticks > 0 else { return }
            let scale = window?.backingScaleFactor ?? 2
            let loc = convert(event.locationInWindow, from: nil)
            let col = min(max(1, Int(loc.x * scale / renderer.cellPixelWidth) + 1), termCols)
            let row = min(max(1, Int((bounds.height - loc.y) * scale / renderer.cellPixelHeight) + 1), termRows)
            let b = dy > 0 ? 64 : 65
            var bytes: [UInt8] = []
            for _ in 0..<ticks { bytes += Array("\u{1b}[<\(b);\(col);\(row)M".utf8) }
            onInput(bytes)
            return
        }

        let threshold: CGFloat = 4
        var lines = 0
        while abs(scrollAccum) >= threshold, abs(lines) < 24 {
            lines += dy > 0 ? 1 : -1
            scrollAccum += scrollAccum > 0 ? -threshold : threshold
        }
        renderer.scrollLines(lines)
    }

    // MARK: - Keyboard

    override func keyDown(with event: NSEvent) {
        if event.modifierFlags.contains(.command) {
            switch event.charactersIgnoringModifiers?.lowercased() {
            case "c":
                if renderer.hasSelection {
                    renderer.selectionText { s in
                        guard !s.isEmpty else { return }
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(s, forType: .string)
                    }
                }
                return
            case "v": if let s = NSPasteboard.general.string(forType: .string) { onInput(Array(s.utf8)) }; return
            case "k": onInput([0x0c]); return   // clear, like the SwiftTerm panes
            default: return   // don't forward other Cmd shortcuts to the pty
            }
        }
        let bytes = Self.encodeKey(event)
        if !bytes.isEmpty { onInput(bytes) }
    }

    static func encodeKey(_ e: NSEvent) -> [UInt8] {
        switch e.keyCode {
        case 36, 76: return [0x0d]
        case 48: return [0x09]
        case 51: return [0x7f]
        case 53: return [0x1b]
        case 123: return [0x1b, 0x5b, 0x44]
        case 124: return [0x1b, 0x5b, 0x43]
        case 126: return [0x1b, 0x5b, 0x41]
        case 125: return [0x1b, 0x5b, 0x42]
        default: break
        }
        guard let chars = e.charactersIgnoringModifiers, let first = chars.unicodeScalars.first else { return [] }
        if e.modifierFlags.contains(.control) {
            let v = first.value
            if v >= 0x61 && v <= 0x7a { return [UInt8(v - 0x60)] }
            if v >= 0x41 && v <= 0x5a { return [UInt8(v - 0x40)] }
        }
        return Array((e.characters ?? "").utf8)
    }

    // MARK: - Theme

    static func themeColors(_ mode: ThemeMode) -> (fg: SIMD4<Float>, bg: SIMD4<Float>) {
        let pal: Palette = mode == .light ? Theme.light : (mode == .sepia ? Theme.sepia : Theme.dark)
        return (simd4(pal.fg), simd4(pal.bg))
    }

    private static func simd4(_ color: SwiftUI.Color) -> SIMD4<Float> {
        let ns = NSColor(color).usingColorSpace(.sRGB) ?? .black
        return SIMD4(Float(ns.redComponent), Float(ns.greenComponent), Float(ns.blueComponent), 1)
    }
}
