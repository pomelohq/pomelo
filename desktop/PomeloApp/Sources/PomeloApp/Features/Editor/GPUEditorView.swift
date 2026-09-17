import SwiftUI
import AppKit
import QuartzCore
import CEditorKit

// Embeds the Rust/wgpu editor kit (libpomelo_editor_kit) in a CAMetalLayer-backed NSView. The Rust side owns all
// rendering; AppKit forwards size + input and asks it to redraw. This is the cross-platform GPU editor path that
// will eventually replace the AppKit NSTextView editor.
final class GPUEditorNSView: NSView {
    private var editor: OpaquePointer?
    private var link: CADisplayLink?
    private var blinkTimer: Timer?
    private var caretOn = true
    var initialText: String = ""
    var languageExt: String = ""
    var tabTitle: String = ""

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layerContentsRedrawPolicy = .duringViewResize
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) unavailable") }

    override func makeBackingLayer() -> CALayer { CAMetalLayer() }

    override var acceptsFirstResponder: Bool { true }
    override var isFlipped: Bool { true }

    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        ensureEditor()
        if window == nil {
            link?.invalidate(); link = nil
            stopBlink()
        } else if link == nil {
            let l = displayLink(target: self, selector: #selector(step))
            l.add(to: .current, forMode: .common)
            link = l
        }
    }

    @objc private func step(_ sender: CADisplayLink) { render() }

    // Zed shows the caret only in the focused editor and blinks it at 500ms.
    override func becomeFirstResponder() -> Bool {
        wakeCaret()
        return super.becomeFirstResponder()
    }

    override func resignFirstResponder() -> Bool {
        stopBlink()
        if let ed = editor { caretOn = false; pomelo_editor_set_caret_on(ed, false) }
        return super.resignFirstResponder()
    }

    private func startBlink() {
        guard blinkTimer == nil else { return }
        let t = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in self?.toggleCaret() }
        RunLoop.main.add(t, forMode: .common)
        blinkTimer = t
    }

    private func stopBlink() {
        blinkTimer?.invalidate()
        blinkTimer = nil
    }

    private func toggleCaret() {
        guard let ed = editor else { return }
        caretOn.toggle()
        pomelo_editor_set_caret_on(ed, caretOn)
    }

    private func wakeCaret() {
        guard let ed = editor else { return }
        caretOn = true
        pomelo_editor_set_caret_on(ed, true)
        stopBlink()
        startBlink()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        metalLayer.contentsScale = window?.backingScaleFactor ?? 2
        resizeEditor()
    }

    override func layout() {
        super.layout()
        if editor == nil { ensureEditor() } else { resizeEditor() }
    }

    private func pixelSize() -> (UInt32, UInt32, CGFloat) {
        let scale = window?.backingScaleFactor ?? 2
        let w = max(1, Int(bounds.width * scale))
        let h = max(1, Int(bounds.height * scale))
        return (UInt32(w), UInt32(h), scale)
    }

    private func ensureEditor() {
        guard editor == nil, window != nil, bounds.width > 1, bounds.height > 1 else { return }
        let (w, h, scale) = pixelSize()
        metalLayer.contentsScale = scale
        let layerPtr = Unmanaged.passUnretained(metalLayer).toOpaque()
        editor = pomelo_editor_new(layerPtr, w, h, Float(scale))
        guard let ed = editor else { return }
        setLanguage(ed, languageExt)
        setTabTitle(ed, tabTitle)
        setText(ed, initialText)
        caretOn = false
        pomelo_editor_set_caret_on(ed, false)
        if window?.firstResponder === self { wakeCaret() }
        render()
    }

    private func resizeEditor() {
        guard let ed = editor, bounds.width > 1, bounds.height > 1 else { return }
        let (w, h, _) = pixelSize()
        pomelo_editor_resize(ed, w, h)
        render()
    }

    private func setText(_ ed: OpaquePointer, _ text: String) {
        let bytes = Array(text.utf8)
        bytes.withUnsafeBufferPointer { pomelo_editor_set_text(ed, $0.baseAddress, $0.count) }
    }

    private func setLanguage(_ ed: OpaquePointer, _ ext: String) {
        let bytes = Array(ext.utf8)
        bytes.withUnsafeBufferPointer { pomelo_editor_set_language(ed, $0.baseAddress, $0.count) }
    }

    private func setTabTitle(_ ed: OpaquePointer, _ title: String) {
        let bytes = Array(title.utf8)
        bytes.withUnsafeBufferPointer { pomelo_editor_set_tab_title(ed, $0.baseAddress, $0.count) }
    }

    func load(_ text: String, ext: String, title: String) {
        initialText = text
        languageExt = ext
        tabTitle = title
        guard let ed = editor else { return }
        setLanguage(ed, ext)
        setTabTitle(ed, title)
        setText(ed, text)
        render()
    }

    private func render() {
        guard let ed = editor else { return }
        pomelo_editor_render(ed)
    }

    override func keyDown(with event: NSEvent) {
        guard let ed = editor else { return super.keyDown(with: event) }
        switch event.keyCode {
        case 51: pomelo_editor_key(ed, 1)
        case 36, 76: pomelo_editor_key(ed, 2)
        case 123: pomelo_editor_key(ed, 3)
        case 124: pomelo_editor_key(ed, 4)
        case 126: pomelo_editor_key(ed, 5)
        case 125: pomelo_editor_key(ed, 6)
        default:
            if let chars = event.characters, !chars.isEmpty {
                let bytes = Array(chars.utf8)
                bytes.withUnsafeBufferPointer { pomelo_editor_insert_text(ed, $0.baseAddress, $0.count) }
            }
        }
        wakeCaret()
        render()
    }

    override func scrollWheel(with event: NSEvent) {
        guard let ed = editor else { return }
        pomelo_editor_scroll(ed, Float(event.scrollingDeltaY))
        render()
    }

    override func mouseDown(with event: NSEvent) {
        guard let ed = editor else { return }
        window?.makeFirstResponder(self)
        let p = convert(event.locationInWindow, from: nil)
        pomelo_editor_click(ed, Float(p.x), Float(p.y))
        wakeCaret()
        render()
    }

    override func mouseDragged(with event: NSEvent) {
        guard let ed = editor else { return }
        let p = convert(event.locationInWindow, from: nil)
        pomelo_editor_drag(ed, Float(p.x), Float(p.y))
        render()
    }

    deinit {
        link?.invalidate()
        blinkTimer?.invalidate()
        if let ed = editor { pomelo_editor_free(ed) }
    }
}

struct GPUEditorView: NSViewRepresentable {
    var text: String
    var ext: String = ""
    var title: String = ""

    func makeNSView(context: Context) -> GPUEditorNSView {
        let v = GPUEditorNSView(frame: .zero)
        v.initialText = text
        v.languageExt = ext
        v.tabTitle = title
        DispatchQueue.main.async { v.window?.makeFirstResponder(v) }
        return v
    }

    func updateNSView(_ nsView: GPUEditorNSView, context: Context) {
        nsView.load(text, ext: ext, title: title)
    }
}

struct GPUEditorSpike: View {
    private static let sample: String = {
        var s = "// pomelo-editor-kit demo (wgpu + glyphon + tree-sitter)\n"
        s += "import { useMemo, useState } from \"react\";\n\n"
        for i in 0..<12 {
            s += "export const Widget\(i) = ({ rows }: { rows: number[] }) => {\n"
            s += "    const [open, setOpen] = useState(false);\n"
            s += "    const total = useMemo(() => rows.reduce((a, b) => a + b, 0), [rows]);\n"
            s += "    // row \(i): renders the running total\n"
            s += "    return <div className=\"widget\">{open ? total : 0}</div>;\n"
            s += "};\n\n"
        }
        return s
    }()

    var body: some View {
        GPUEditorView(text: Self.sample, ext: "tsx", title: "Widget.tsx")
            .background(Color(red: 40.0 / 255.0, green: 44.0 / 255.0, blue: 51.0 / 255.0))
    }
}

struct OpenGPUEditorButton: View {
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Button("GPU Editor Spike") { openWindow(id: "gpu-editor-spike") }
            .keyboardShortcut("g", modifiers: [.command, .option, .control])
    }
}

struct GPUEditorFilesToggle: View {
    @AppStorage("gpuEditor") private var on = false
    var body: some View {
        Toggle("GPU Editor in Files (test)", isOn: $on)
            .keyboardShortcut("g", modifiers: [.command, .shift, .option])
    }
}
