#if os(macOS)
import AppKit
import QuartzCore
import CoreText
import SwiftTerm

// AppKit host around the shared `TerminalRenderer`. Forwards NSEvent input (keys, mouse,
// scroll, selection) to the renderer's semantic API. App-specific wiring (theme mapping,
// PTY stream) lives in the app's SwiftUI representable; this view is app-agnostic.
public final class MetalTerminalHostView: NSView {
    public let renderer = TerminalRenderer()
    private let statsLabel = NSTextField(labelWithString: "")
    private var displayLink: CADisplayLink?

    public var terminal: Terminal { renderer.terminal }
    public var termCols: Int { renderer.termCols }
    public var termRows: Int { renderer.termRows }
    public var onResize: ((Int, Int) -> Void)?
    public var onInput: ([UInt8]) -> Void = { _ in }
    public var statsEnabled: Bool = false {
        didSet {
            renderer.statsEnabled = statsEnabled
            if statsLabel.isHidden != !statsEnabled { statsLabel.isHidden = !statsEnabled }
        }
    }

    public override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer = renderer.metalLayer
        renderer.onResize = { [weak self] c, r in self?.onResize?(c, r) }
        renderer.onStats = { [weak self] s in self?.statsLabel.stringValue = s }
        statsLabel.font = .monospacedSystemFont(ofSize: 11, weight: .semibold)
        statsLabel.textColor = .systemGreen
        statsLabel.backgroundColor = NSColor.black.withAlphaComponent(0.85)
        statsLabel.drawsBackground = true
        statsLabel.frame = NSRect(x: 8, y: 8, width: 620, height: 18)
        statsLabel.isHidden = true
        addSubview(statsLabel)
    }
    public required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    public func feed(_ bytes: [UInt8]) { renderer.feed(bytes) }
    public func feed(_ bytes: ArraySlice<UInt8>) { renderer.feed(bytes) }

    public func setFont(family: String, size: CGFloat) {
        renderer.setFont(family: family, size: size)
        needsLayout = true
    }

    public func applyColors(fg: SIMD4<Float>, bg: SIMD4<Float>) {
        renderer.setColors(fg: fg, bg: bg)
    }

    public override func viewDidMoveToWindow() {
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

    public override func layout() {
        super.layout()
        let scale = window?.backingScaleFactor ?? 2
        statsLabel.frame = NSRect(x: 8, y: bounds.height - 24, width: 620, height: 18)
        renderer.updateGeometry(pointSize: bounds.size, scale: scale)
    }

    // MARK: - Focus

    public override var acceptsFirstResponder: Bool { true }
    public override func becomeFirstResponder() -> Bool { renderer.setFocused(true); return super.becomeFirstResponder() }
    public override func resignFirstResponder() -> Bool { renderer.setFocused(false); return super.resignFirstResponder() }

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

    public override func mouseDown(with event: NSEvent) {
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

    public override func mouseDragged(with event: NSEvent) {
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

    public override func mouseUp(with event: NSEvent) {
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

    public override func scrollWheel(with event: NSEvent) {
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

    public override func keyDown(with event: NSEvent) {
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
}
#endif
