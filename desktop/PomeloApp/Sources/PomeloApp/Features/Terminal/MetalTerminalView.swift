import AppKit
import SwiftUI
import Metal
import CoreText
import SwiftTerm

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

// GPU terminal renderer spike. It reuses SwiftTerm's headless `Terminal` (the VT
// parser + grid buffer) and only replaces the drawing: instead of CoreText laying
// out every glyph each frame (the CPU cost under a log firehose), each unique glyph
// is rasterized once into a texture atlas and the screen is drawn as one instanced
// pass of textured quads. This is the Alacritty/Ghostty rendering model, ported to
// Swift over our existing terminal engine. Verify visually + measure CPU vs SwiftTerm.

struct CellInstance {
    var gridPos: SIMD2<Float> = .zero      // col, row
    var uvOrigin: SIMD2<Float> = .zero     // glyph tile top-left in atlas (0..1)
    var uvSize: SIMD2<Float> = .zero       // glyph tile size in atlas (0..1)
    var fg: SIMD4<Float> = .zero
    var bg: SIMD4<Float> = .zero
    var underline: Float = 0
    var cellW: Float = 1     // quad width in cells (2 for a wide glyph)
}

struct TermUniforms {
    var viewportPx: SIMD2<Float> = .zero
    var cellPx: SIMD2<Float> = .zero
}

// Produced on the parse queue (no atlas/GPU touch); the main thread resolves each
// glyph against the atlas. Keeps the atlas single-threaded so its texture writes
// never race the GPU read.
struct CellDesc {
    var gridPos: SIMD2<Float> = .zero
    var fg: SIMD4<Float> = .zero
    var bg: SIMD4<Float> = .zero
    var ch: Character? = nil
    var style: Int = 0    // 0 regular, |1 bold, |2 italic, |4 underline
    var wide = false      // CJK / emoji / nerd-font: the glyph spans two cells
}

// One tile per unique scalar, cell-sized, so the fragment shader samples a glyph by
// the cell's local uv with no per-glyph bearing math.
final class GlyphAtlas {
    let texture: MTLTexture
    let tilePx: SIMD2<Int>
    private let cols: Int
    private var next = 0
    private var map: [String: (origin: SIMD2<Float>, size: SIMD2<Float>)] = [:]
    let uvSize: SIMD2<Float>
    private let font: CTFont
    private let boldFont: CTFont
    private let italicFont: CTFont
    private let boldItalicFont: CTFont
    private let descent: CGFloat

    init?(device: MTLDevice, font: CTFont, cellW: Int, cellH: Int, atlasSide: Int = 4096) {
        let sz = CTFontGetSize(font)
        self.font = font
        self.boldFont = CTFontCreateCopyWithSymbolicTraits(font, sz, nil, .boldTrait, .boldTrait) ?? font
        self.italicFont = CTFontCreateCopyWithSymbolicTraits(font, sz, nil, .italicTrait, .italicTrait) ?? font
        self.boldItalicFont = CTFontCreateCopyWithSymbolicTraits(font, sz, nil, [.boldTrait, .italicTrait], [.boldTrait, .italicTrait]) ?? font
        self.descent = CTFontGetDescent(font)
        self.tilePx = SIMD2(cellW, cellH)
        self.cols = max(1, atlasSide / cellW)
        self.uvSize = SIMD2(Float(cellW) / Float(atlasSide), Float(cellH) / Float(atlasSide))
        let desc = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .r8Unorm, width: atlasSide, height: atlasSide, mipmapped: false)
        desc.usage = [.shaderRead]
        guard let tex = device.makeTexture(descriptor: desc) else { return nil }
        self.texture = tex
    }

    func glyph(for ch: Character, bold: Bool = false, italic: Bool = false, wide: Bool = false) -> (origin: SIMD2<Float>, size: SIMD2<Float>) {
        let key = (bold ? "\u{1}" : "") + (italic ? "\u{2}" : "") + (wide ? "\u{3}" : "") + String(ch)
        if let g = map[key] { return g }
        PerfHUD.shared.tick("term:raster")
        let tiles = wide ? 2 : 1
        if wide && next % cols == cols - 1 { next += 1 }   // keep both tiles on one row
        let slot = next; next += tiles
        let cx = slot % cols, cy = slot / cols
        let px = cx * tilePx.x, py = cy * tilePx.y
        rasterize(ch, bold: bold, italic: italic, wide: wide, intoTileAt: px, py)
        let g = (origin: SIMD2(Float(px) / Float(texture.width), Float(py) / Float(texture.height)),
                 size: SIMD2(uvSize.x * Float(tiles), uvSize.y))
        map[key] = g
        return g
    }

    private func hasGlyph(_ f: CTFont, _ units: [UniChar]) -> Bool {
        var g = [CGGlyph](repeating: 0, count: units.count)
        _ = CTFontGetGlyphsForCharacters(f, units, &g, units.count)
        // For an astral codepoint (surrogate pair) the glyph lands in g[0] and the low
        // surrogate maps to 0, so check the first glyph — not every unit.
        return (g.first ?? 0) != 0
    }

    // CTLine draw at the text baseline; auto-cascades to a system font for glyphs the
    // base font lacks (emoji, symbols).
    private func drawText(_ ch: Character, font f: CTFont, ctx: CGContext) {
        let attrs = [kCTFontAttributeName: f,
                     kCTForegroundColorAttributeName: CGColor(red: 1, green: 1, blue: 1, alpha: 1)] as CFDictionary
        guard let astr = CFAttributedStringCreate(nil, String(ch) as CFString, attrs) else { return }
        let line = CTLineCreateWithAttributedString(astr)
        ctx.textPosition = CGPoint(x: 0, y: descent)
        CTLineDraw(line, ctx)
    }

    // Nerd Font / icon glyphs live in the Private Use Areas. They carry metrics that
    // don't sit at the text baseline, so fit them into the cell instead of baseline-drawing.
    static func isIconGlyph(_ v: UInt32) -> Bool {
        (0xE000...0xF8FF).contains(v) || (0xF0000...0x10FFFD).contains(v)
    }

    // Scale + center the glyph to fill the cell (like Alacritty/Ghostty do for icons),
    // so devicons render fully instead of clipping to blank. Returns false if it can't
    // resolve the glyph (caller falls back to the normal path).
    private func drawIconFit(_ ch: Character, font f: CTFont, ctx: CGContext, w: Int, h: Int) -> Bool {
        let units = Array(String(ch).utf16)
        var glyphs = [CGGlyph](repeating: 0, count: units.count)
        _ = CTFontGetGlyphsForCharacters(f, units, &glyphs, units.count)   // astral: glyph in [0], low surrogate 0
        guard let g = glyphs.first, g != 0 else { return false }
        var bbox = CGRect.zero
        _ = withUnsafePointer(to: g) { CTFontGetBoundingRectsForGlyphs(f, .horizontal, $0, &bbox, 1) }
        guard bbox.width > 0.5, bbox.height > 0.5 else { return false }
        let pad = CGFloat(w) * 0.06
        let scale = min((CGFloat(w) - 2 * pad) / bbox.width, (CGFloat(h) - 2 * pad) / bbox.height)
        let tx = (CGFloat(w) - bbox.width * scale) / 2 - bbox.minX * scale
        let ty = (CGFloat(h) - bbox.height * scale) / 2 - bbox.minY * scale
        ctx.saveGState()
        ctx.setFillColor(red: 1, green: 1, blue: 1, alpha: 1)
        ctx.translateBy(x: tx, y: ty); ctx.scaleBy(x: scale, y: scale)
        var pos = CGPoint.zero
        withUnsafePointer(to: g) { gp in withUnsafePointer(to: pos) { pp in CTFontDrawGlyphs(f, gp, pp, 1, ctx) } }
        ctx.restoreGState()
        return true
    }

    private func rasterize(_ ch: Character, bold: Bool, italic: Bool, wide: Bool, intoTileAt px: Int, _ py: Int) {
        let w = tilePx.x * (wide ? 2 : 1), h = tilePx.y
        // CTLine (which shapes the whole grapheme, incl. Vietnamese/combining marks)
        // needs a color context; it does not draw in a gray-only one. Rasterize white
        // text on black in RGBA, then keep the red channel as the R8 coverage.
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return }
        ctx.setFillColor(red: 0, green: 0, blue: 0, alpha: 1); ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        let base = bold && italic ? boldItalicFont : bold ? boldFont : italic ? italicFont : font
        let units = Array(String(ch).utf16)
        let scalar = ch.unicodeScalars.first?.value ?? 0
        if ch.unicodeScalars.count == 1, Self.isIconGlyph(scalar) {
            // Nerd Font icon: draw with the base font, or a fallback that has it, scaled
            // + centered to fit the cell (Ghostty's glyph-constraint approach).
            let iconFont = hasGlyph(base, units) ? base
                : CTFontCreateForString(base, String(ch) as CFString, CFRange(location: 0, length: units.count))
            if hasGlyph(iconFont, units), drawIconFit(ch, font: iconFont, ctx: ctx, w: w, h: h) {
                // constrained icon drawn
            } else {
                drawText(ch, font: base, ctx: ctx)
            }
        } else {
            // Text / symbols: CTLine draws with the base font and auto-cascades to a
            // system font for anything it lacks (emoji, the agent statusline glyphs).
            drawText(ch, font: base, ctx: ctx)
        }
        guard let data = ctx.data else { return }
        let rgba = data.bindMemory(to: UInt8.self, capacity: w * h * 4)
        var cov = [UInt8](repeating: 0, count: w * h)
        for i in 0..<(w * h) { cov[i] = rgba[i * 4] }
        cov.withUnsafeBytes {
            texture.replace(region: MTLRegionMake2D(px, py, w, h), mipmapLevel: 0, withBytes: $0.baseAddress!, bytesPerRow: w)
        }
    }
}

final class MetalTerminalView: NSView {
    // Device + shader library + pipeline are identical for every terminal and the
    // shader compile (makeLibrary from source) costs ~1s on the main thread, so a new
    // terminal per workspace switch would re-pay it. Compile once, share across views.
    private static let sharedDevice = MTLCreateSystemDefaultDevice()!
    private static let sharedPipeline: MTLRenderPipelineState = {
        let lib = try! sharedDevice.makeLibrary(source: MetalTerminalView.shaderSource, options: nil)
        let desc = MTLRenderPipelineDescriptor()
        desc.vertexFunction = lib.makeFunction(name: "term_vertex")
        desc.fragmentFunction = lib.makeFunction(name: "term_fragment")
        desc.colorAttachments[0].pixelFormat = .bgra8Unorm
        desc.colorAttachments[0].isBlendingEnabled = true
        desc.colorAttachments[0].rgbBlendOperation = .add
        desc.colorAttachments[0].alphaBlendOperation = .add
        desc.colorAttachments[0].sourceRGBBlendFactor = .sourceAlpha
        desc.colorAttachments[0].sourceAlphaBlendFactor = .sourceAlpha
        desc.colorAttachments[0].destinationRGBBlendFactor = .oneMinusSourceAlpha
        desc.colorAttachments[0].destinationAlphaBlendFactor = .oneMinusSourceAlpha
        return try! sharedDevice.makeRenderPipelineState(descriptor: desc)
    }()

    private let device = MetalTerminalView.sharedDevice
    private let queue: MTLCommandQueue
    private let metalLayer = CAMetalLayer()
    private var pipeline: MTLRenderPipelineState!
    private var atlas: GlyphAtlas!
    private var instances: [CellInstance] = []
    private var instanceBuf: MTLBuffer?
    private var uniforms = TermUniforms()
    private var cellPx = SIMD2<Float>(8, 16)
    // The VT parse + cell/instance build run here, off the main thread; main only
    // uploads the prepared instances to the GPU and presents. The `terminal` engine
    // and the glyph atlas are touched only on this queue.
    private let parseQueue = DispatchQueue(label: "pom.term.parse")
    // Present is driven by a display link at vsync (not per-feed) so rapid scroll
    // output can't present a torn frame; main keeps only the latest descriptors.
    private var displayLink: CADisplayLink?
    private var latestDescs: [CellDesc]?
    private var needsPresent = false
    // parseQueue-owned persistent grid so only rows the terminal marks dirty are
    // rebuilt; unchanged rows skip the per-cell getCharData/color work.
    private var grid: [CellDesc] = []
    private var gridCols = 0, gridRows = 0

    let terminal: Terminal
    private let palette: [SIMD4<Float>]
    private(set) var termCols = 80, termRows = 24
    var onResize: ((Int, Int) -> Void)?
    var onInput: ([UInt8]) -> Void = { _ in }

    // Default fg/bg follow the app theme (matches SwiftTerm's nativeForeground/Background).
    // defFg/defBg are read on parseQueue (fillRows); clearColor on main (uploadAndRender).
    private var defFg = SIMD4<Float>(0.85, 0.85, 0.85, 1)
    private var defBg = SIMD4<Float>(0.08, 0.08, 0.09, 1)
    private var clearColor = MTLClearColor(red: 0.08, green: 0.08, blue: 0.09, alpha: 1)
    private let themeDelegate: HeadlessTerminalDelegate

    // Meter: main-thread ms/frame is what "smoothness" actually costs; parse ms is the
    // work now off the main thread. FPS = present rate.
    private let statsLabel = NSTextField(labelWithString: "")
    private var lastParseMs: Double = 0
    private var frameCount = 0
    private var lastStatsAt = CACurrentMediaTime()
    private var fedBytes = 0

    override init(frame frameRect: NSRect) {
        queue = device.makeCommandQueue()!
        palette = Self.ansi256Palette()
        let delegate = HeadlessTerminalDelegate()
        themeDelegate = delegate
        terminal = Terminal(delegate: delegate)
        super.init(frame: frameRect)
        delegate.onChange = {}   // feed() drives the rebuild + present
        applyTheme(activeThemeMode)
        wantsLayer = true
        layer = metalLayer
        metalLayer.device = device
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = true
        setupFontAndAtlas()
        setupPipeline()
        statsLabel.font = .monospacedSystemFont(ofSize: 11, weight: .semibold)
        statsLabel.textColor = .systemGreen
        statsLabel.backgroundColor = NSColor.black.withAlphaComponent(0.85)
        statsLabel.drawsBackground = true
        statsLabel.frame = NSRect(x: 8, y: 8, width: 620, height: 18)
        statsLabel.isHidden = !UserDefaults.standard.bool(forKey: Self.statsKey)
        addSubview(statsLabel)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    private var fontSize: CGFloat = 12

    // The configured monospace font family (empty = SF Mono). Pick a Nerd Font here to
    // get devicon glyphs; unlike a cascade fallback, the whole font's metrics are used
    // so icons sit in the cell correctly.
    private var fontFamily = ""

    func setFont(family: String, size: CGFloat) {
        guard family != fontFamily || abs(size - fontSize) > 0.1 else { return }
        fontFamily = family; fontSize = size
        setupFontAndAtlas()
        lastYDisp = -1
        needsLayout = true
    }

    static func monoFont(_ size: CGFloat, family: String) -> CTFont {
        if !family.isEmpty {
            let d = NSFontDescriptor(fontAttributes: [.family: family])
            if let f = NSFont(descriptor: d, size: size) { return f as CTFont }
        }
        return NSFont.monospacedSystemFont(ofSize: size, weight: .regular) as CTFont
    }

    // Families for the Settings picker: monospace fonts, plus every Nerd Font (the
    // non-"Mono" Nerd variants aren't flagged monospace but are the ones people run in
    // a terminal for full icon coverage).
    static func monospaceFamilies() -> [String] {
        let coll = CTFontCollectionCreateFromAvailableFonts(nil)
        let all = (CTFontCollectionCreateMatchingFontDescriptors(coll) as? [CTFontDescriptor]) ?? []
        var out = Set<String>()
        for d in all {
            guard let fam = CTFontDescriptorCopyAttribute(d, kCTFontFamilyNameAttribute) as? String, !fam.hasPrefix(".") else { continue }
            if fam.lowercased().contains("nerd font") { out.insert(fam); continue }
            let traits = (CTFontDescriptorCopyAttribute(d, kCTFontTraitsAttribute) as? [CFString: Any]) ?? [:]
            let sym = (traits[kCTFontSymbolicTrait] as? UInt32) ?? 0
            if sym & CTFontSymbolicTraits.traitMonoSpace.rawValue != 0 { out.insert(fam) }
        }
        return out.sorted()
    }

    private func setupFontAndAtlas() {
        let font = Self.monoFont(fontSize, family: fontFamily)
        let advance = self.advanceWidth(font)
        let ascent = CTFontGetAscent(font), descent = CTFontGetDescent(font), leading = CTFontGetLeading(font)
        let scale = Float(window?.backingScaleFactor ?? 2)
        let cw = Int((advance * CGFloat(scale)).rounded(.up))
        let ch = Int(((ascent + descent + leading) * CGFloat(scale)).rounded(.up))
        cellPx = SIMD2(Float(cw), Float(ch))
        let scaledFont = Self.monoFont(fontSize * CGFloat(scale), family: fontFamily)
        // Shared per (cellW,cellH): every terminal has the same font/scale, so a new
        // terminal (workspace switch) or a burst of varied text reuses already-
        // rasterized glyphs instead of re-paying ~1-2ms of CTLine draw each on main.
        atlas = Self.sharedAtlas(font: scaledFont, cellW: cw, cellH: ch)
        // Pre-rasterize printable ASCII so the parse queue never writes the atlas
        // texture mid-frame while the GPU samples it (no-op once the shared atlas is warm).
        for u in 32...126 { if let s = UnicodeScalar(u) { _ = atlas.glyph(for: Character(s)) } }
    }

    private static var atlasCache: [String: GlyphAtlas] = [:]
    private static func sharedAtlas(font: CTFont, cellW: Int, cellH: Int) -> GlyphAtlas {
        // Key by the font too: a different family at the same cell size must not reuse
        // another font's rasterized glyphs.
        let name = (CTFontCopyPostScriptName(font) as String)
        let key = "\(name)|\(cellW)x\(cellH)"
        if let a = atlasCache[key] { return a }
        let a = GlyphAtlas(device: sharedDevice, font: font, cellW: cellW, cellH: cellH)!
        atlasCache[key] = a
        return a
    }

    private func advanceWidth(_ font: CTFont) -> CGFloat {
        var g = CGGlyph(0); var ch: UniChar = UniChar("M".unicodeScalars.first!.value)
        CTFontGetGlyphsForCharacters(font, &ch, &g, 1)
        var adv = CGSize.zero
        CTFontGetAdvancesForGlyphs(font, .horizontal, &g, &adv, 1)
        return adv.width
    }

    private func setupPipeline() {
        pipeline = Self.sharedPipeline   // compiled once (static); see sharedPipeline
    }

    func feed(_ bytes: [UInt8]) {
        fedBytes += bytes.count
        parseQueue.async { [weak self] in
            guard let self else { return }
            let before = self.terminal.buffer.yDisp
            self.terminal.feed(byteArray: bytes)
            // SwiftTerm snaps the viewport to the live bottom on output (its userScrolling
            // flag is module-internal). If the user has scrolled up, restore their position
            // so streaming output doesn't yank them to the bottom.
            self.scrollBottomYDisp = self.terminal.buffer.yDisp
            if self.userScrolledUp { self.terminal.buffer.yDisp = min(before, self.scrollBottomYDisp) }
            let t0 = CACurrentMediaTime()
            let descs = self.buildDescs()
            let yd = self.terminal.buffer.yDisp
            self.lastParseMs = (CACurrentMediaTime() - t0) * 1000
            DispatchQueue.main.async { self.latestDescs = descs; self.viewYDisp = yd; self.needsPresent = true }
        }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil, displayLink == nil {
            let link = displayLink(target: self, selector: #selector(onDisplayLink))
            link.add(to: .main, forMode: .common)
            displayLink = link
        } else if window == nil {
            displayLink?.invalidate(); displayLink = nil
            setAutoscroll(0)
        }
    }

    @objc private func onDisplayLink(_ link: CADisplayLink) {
        guard needsPresent, let descs = latestDescs else { return }
        needsPresent = false
        PerfHUD.shared.tick("term:present")
        applyDescs(descs)
        uploadAndRender()
    }

    override var acceptsFirstResponder: Bool { true }
    override func becomeFirstResponder() -> Bool { isFocused = true; refreshCursor(); return super.becomeFirstResponder() }
    override func resignFirstResponder() -> Bool { isFocused = false; refreshCursor(); return super.resignFirstResponder() }
    private func refreshCursor() {
        parseQueue.async { [weak self] in
            guard let self else { return }
            let descs = self.buildDescs()
            DispatchQueue.main.async { self.latestDescs = descs; self.needsPresent = true }
        }
    }

    private enum DragKind { case none, select, app }
    private var dragKind: DragKind = .none
    private var mouseModeOn: Bool {
        switch terminal.mouseMode { case .off: return false; default: return true }
    }

    // When a full-screen TUI (Claude) enables mouse reporting it owns the mouse, so
    // forward clicks/drags to it; hold Option to grab a terminal text selection instead.
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        if mouseModeOn && !event.modifierFlags.contains(.option) {
            dragKind = .app; sendMouseSGR(event, code: 0, press: true)
            return
        }
        let (col, row) = cell(at: event)
        if event.clickCount == 2 {                    // double-click: word
            let (lo, hi) = wordRange(col: col, row: row)
            selStart = (lo, row + viewYDisp); selEnd = (hi, row + viewYDisp)
            dragKind = .none; needsPresent = true
            return
        }
        if event.clickCount >= 3 {                    // triple-click: whole line
            selStart = (0, row + viewYDisp); selEnd = (max(0, termCols - 1), row + viewYDisp)
            dragKind = .none; needsPresent = true
            return
        }
        dragKind = .select; let c = selCell(at: event); selStart = c; selEnd = c; needsPresent = true
    }

    private func wordRange(col: Int, row: Int) -> (Int, Int) {
        func isWord(_ c: Int) -> Bool {
            guard c >= 0, c < terminal.cols, let cd = terminal.getCharData(col: c, row: row) else { return false }
            let ch = cd.getCharacter()
            return ch != " " && ch != "\t" && ch != "\u{0}"
        }
        guard isWord(col) else { return (col, col) }
        var lo = col, hi = col
        while lo > 0, isWord(lo - 1) { lo -= 1 }
        while hi < terminal.cols - 1, isWord(hi + 1) { hi += 1 }
        return (lo, hi)
    }
    // Absolute buffer cell (screen row offset by the current viewport) so a selection
    // sticks to its text while the viewport scrolls.
    private func selCell(at event: NSEvent) -> (col: Int, row: Int) {
        let c = cell(at: event); return (c.0, c.1 + viewYDisp)
    }
    override func mouseDragged(with event: NSEvent) {
        switch dragKind {
        case .app: sendMouseSGR(event, code: 32, press: true)
        case .select:
            let loc = convert(event.locationInWindow, from: nil)
            let scale = window?.backingScaleFactor ?? 2
            lastDragCol = min(max(0, Int(loc.x * scale / CGFloat(cellPx.x))), max(0, termCols - 1))
            // Dragging past the top/bottom edge auto-scrolls so the selection can
            // extend beyond what's on screen (older lines above, newer below).
            if loc.y > bounds.height { setAutoscroll(-1) }
            else if loc.y < 0 { setAutoscroll(1) }
            else { setAutoscroll(0); selEnd = selCell(at: event); needsPresent = true }
        case .none: break
        }
    }
    override func mouseUp(with event: NSEvent) {
        setAutoscroll(0)
        switch dragKind {
        case .app: sendMouseSGR(event, code: 0, press: false)
        case .select: if let s = selStart, let e = selEnd, s == e { selStart = nil; selEnd = nil; needsPresent = true }
        case .none: break
        }
        dragKind = .none
    }

    private var autoscrollTimer: Timer?
    private var autoscrollDir = 0
    private var lastDragCol = 0

    private func setAutoscroll(_ dir: Int) {
        if dir == 0 {
            autoscrollTimer?.invalidate(); autoscrollTimer = nil; autoscrollDir = 0; return
        }
        if autoscrollDir == dir, autoscrollTimer != nil { return }
        autoscrollDir = dir
        autoscrollTimer?.invalidate()
        let t = Timer(timeInterval: 0.05, repeats: true) { [weak self] _ in self?.autoscrollStep(dir) }
        RunLoop.main.add(t, forMode: .common)
        autoscrollTimer = t
    }

    private func autoscrollStep(_ dir: Int) {
        parseQueue.async { [weak self] in
            guard let self else { return }
            let cur = self.terminal.buffer.yDisp
            let target = max(0, min(cur + dir, self.scrollBottomYDisp))
            guard target != cur else { return }
            self.terminal.buffer.yDisp = target
            self.userScrolledUp = target < self.scrollBottomYDisp
            let descs = self.buildDescs()
            DispatchQueue.main.async {
                self.viewYDisp = target
                let edgeRow = dir < 0 ? 0 : max(0, self.termRows - 1)
                self.selEnd = (self.lastDragCol, edgeRow + target)
                self.latestDescs = descs; self.needsPresent = true
            }
        }
    }

    private func sendMouseSGR(_ event: NSEvent, code: Int, press: Bool) {
        let (c, r) = cell(at: event)
        let col = min(max(1, c + 1), max(1, termCols)), row = min(max(1, r + 1), max(1, termRows))
        onInput(Array("\u{1b}[<\(code);\(col);\(row)\(press ? "M" : "m")".utf8))
    }

    override func keyDown(with event: NSEvent) {
        if event.modifierFlags.contains(.command) {
            switch event.charactersIgnoringModifiers?.lowercased() {
            case "c": if selStart != nil { copySelection() }; return
            case "v": if let s = NSPasteboard.general.string(forType: .string) { onInput(Array(s.utf8)) }; return
            case "k": onInput([0x0c]); return   // clear, like the SwiftTerm panes
            default: return   // don't forward other Cmd shortcuts to the pty
            }
        }
        let bytes = Self.encodeKey(event)
        if !bytes.isEmpty { onInput(bytes) }
    }

    private func copySelection() {
        guard let s = selStart, let e = selEnd, s != e else { return }
        parseQueue.async { [weak self] in
            guard let self else { return }
            let a = self.le(s, e) ? s : e, b = self.le(s, e) ? e : s
            var out = ""
            for row in a.1...b.1 {
                let lo = row == a.1 ? a.0 : 0
                let hi = row == b.1 ? b.0 : self.terminal.cols - 1
                var line = ""
                if lo <= hi { for col in lo...hi { if let cd = self.terminal.getCharData(col: col, row: row) { line += String(cd.getCharacter()) } } }
                out += line.replacingOccurrences(of: "\\s+$", with: "", options: .regularExpression)
                if row < b.1 { out += "\n" }
            }
            DispatchQueue.main.async {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(out, forType: .string)
            }
        }
    }

    private var scrollAccum: CGFloat = 0
    // The live bottom (== buffer.yBase, which isn't public) captured after each feed;
    // scrollback clamps between 0 and this. lastYDisp forces a full refill on a move.
    private var scrollBottomYDisp = 0
    private var userScrolledUp = false   // holds the viewport across streaming output
    private var lastYDisp = -1
    private var isFocused = false         // dims the cursor when the terminal isn't first responder
    // yDisp of the currently-presented frame, mirrored on the main thread so mouse
    // events and selection hit-testing agree with what's on screen.
    private var viewYDisp = 0
    // Selection anchors are absolute buffer rows (screen row + yDisp), so the highlight
    // sticks to the text as the viewport scrolls instead of to fixed screen rows.
    private var selStart: (col: Int, row: Int)?
    private var selEnd: (col: Int, row: Int)?

    private func cell(at event: NSEvent) -> (Int, Int) {
        let scale = window?.backingScaleFactor ?? 2
        let loc = convert(event.locationInWindow, from: nil)
        let col = min(max(0, Int(loc.x * scale / CGFloat(cellPx.x))), max(0, termCols - 1))
        let row = min(max(0, Int((bounds.height - loc.y) * scale / CGFloat(cellPx.y))), max(0, termRows - 1))
        return (col, row)
    }

    private func le(_ p: (Int, Int), _ q: (Int, Int)) -> Bool { p.1 < q.1 || (p.1 == q.1 && p.0 <= q.0) }

    private func selectionContains(_ col: Int, _ row: Int) -> Bool {
        guard let s = selStart, let e = selEnd, s != e else { return false }
        let p = (col, row + viewYDisp)   // screen row -> absolute buffer row
        let a = le(s, e) ? s : e, b = le(s, e) ? e : s
        return le(a, p) && le(p, b)
    }

    override func scrollWheel(with event: NSEvent) {
        let wantsMouse: Bool
        switch terminal.mouseMode { case .off: wantsMouse = false; default: wantsMouse = true }
        let dy = event.scrollingDeltaY
        guard dy != 0 else { return }
        if (dy > 0) != (scrollAccum > 0) { scrollAccum = 0 }   // direction flip: reset
        scrollAccum += dy

        // A full-screen TUI (Claude) owns the mouse: forward wheel ticks as SGR events.
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
            let col = min(max(1, Int(loc.x * scale / CGFloat(cellPx.x)) + 1), termCols)
            let row = min(max(1, Int((bounds.height - loc.y) * scale / CGFloat(cellPx.y)) + 1), termRows)
            let b = dy > 0 ? 64 : 65
            var bytes: [UInt8] = []
            for _ in 0..<ticks { bytes += Array("\u{1b}[<\(b);\(col);\(row)M".utf8) }
            onInput(bytes)
            return
        }

        // Plain shell: move the viewport through scrollback. Wheel up (dy > 0) shows
        // older lines (smaller yDisp).
        let threshold: CGFloat = 4
        var lines = 0
        while abs(scrollAccum) >= threshold, abs(lines) < 24 {
            lines += dy > 0 ? 1 : -1
            scrollAccum += scrollAccum > 0 ? -threshold : threshold
        }
        guard lines != 0 else { return }
        parseQueue.async { [weak self] in
            guard let self else { return }
            let cur = self.terminal.buffer.yDisp
            let target = max(0, min(cur - lines, self.scrollBottomYDisp))
            guard target != cur else { return }
            self.terminal.buffer.yDisp = target
            self.userScrolledUp = target < self.scrollBottomYDisp
            let descs = self.buildDescs()
            DispatchQueue.main.async { self.latestDescs = descs; self.viewYDisp = target; self.needsPresent = true }
        }
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

    override func layout() {
        super.layout()
        let scale = window?.backingScaleFactor ?? 2
        metalLayer.contentsScale = scale
        metalLayer.drawableSize = CGSize(width: bounds.width * scale, height: bounds.height * scale)
        statsLabel.frame = NSRect(x: 8, y: bounds.height - 24, width: 620, height: 18)
        let cols = max(1, Int(bounds.width * scale / CGFloat(cellPx.x)))
        let rows = max(1, Int(bounds.height * scale / CGFloat(cellPx.y)))
        let changed = cols != termCols || rows != termRows
        termCols = cols; termRows = rows
        parseQueue.async { [weak self] in
            guard let self else { return }
            self.terminal.resize(cols: cols, rows: rows)
            let descs = self.buildDescs()
            DispatchQueue.main.async { self.latestDescs = descs; self.needsPresent = true }
        }
        if changed { onResize?(cols, rows) }
    }

    // Runs on parseQueue: refreshes only the dirty rows in the persistent grid, then
    // snapshots it (+ the cursor). Touches only the terminal engine, never the atlas.
    private func buildDescs() -> [CellDesc] {
        let cols = terminal.cols, rows = terminal.rows
        let yd = terminal.buffer.yDisp
        let sizeChanged = gridCols != cols || gridRows != rows || grid.count != cols * rows
        if sizeChanged {
            grid = Array(repeating: CellDesc(), count: cols * rows)
            gridCols = cols; gridRows = rows
        }
        // A viewport move (scrollback or a feed snapping back to the bottom) changes
        // every row, so refill the whole grid — the incremental range won't cover it.
        if sizeChanged || yd != lastYDisp {
            lastYDisp = yd
            fillRows(0, rows - 1)
        } else if let r = terminal.getUpdateRange() {
            fillRows(max(0, r.startY), min(rows - 1, r.endY))
        }
        terminal.clearUpdateRange()
        var out = grid
        if yd == scrollBottomYDisp { out.append(cursorDesc(cols: cols, rows: rows)) }
        return out
    }

    private func fillRows(_ a: Int, _ b: Int) {
        guard a <= b, gridCols > 0 else { return }
        let cols = gridCols
        let defFg = self.defFg, defBg = self.defBg
        for row in a...b {
            for col in 0..<cols {
                var d = CellDesc()
                d.gridPos = SIMD2(Float(col), Float(row))
                if let cd = terminal.getCharData(col: col, row: row), cd.width == 0 {
                    // Second half of a wide glyph: draw nothing so the 2-cell glyph from
                    // the previous column isn't overpainted.
                    d.bg = SIMD4(0, 0, 0, 0)
                    grid[row * cols + col] = d
                    continue
                }
                if let cd = terminal.getCharData(col: col, row: row) {
                    let st = cd.attribute.style
                    if cd.width == 2 { d.wide = true }
                    var fg = color(cd.attribute.fg, def: defFg)
                    var bg = color(cd.attribute.bg, def: defBg)
                    if st.contains(.inverse) { swap(&fg, &bg) }
                    if st.contains(.dim) { fg = SIMD4(fg.x * 0.6, fg.y * 0.6, fg.z * 0.6, fg.w) }
                    d.fg = fg; d.bg = bg
                    if st.contains(.bold) { d.style |= 1 }
                    if st.contains(.italic) { d.style |= 2 }
                    if st.contains(.underline) { d.style |= 4 }
                    if st.contains(.crossedOut) { d.style |= 8 }
                    let ch = terminal.getCharacter(for: cd)   // NOT cd.getCharacter(): astral scalars (Material Design nerd icons, U+F0000+) are stored via the terminal's grapheme-index map; the CharData-only getter returns a space for them
                    if ch != " ", let scalar = ch.unicodeScalars.first, scalar.value != 0 { d.ch = ch }
                } else {
                    d.bg = defBg
                }
                grid[row * cols + col] = d
            }
        }
    }

    private func cursorDesc(cols: Int, rows: Int) -> CellDesc {
        var c = CellDesc()
        c.gridPos = SIMD2(Float(min(max(0, terminal.buffer.x), cols - 1)),
                          Float(min(max(0, terminal.buffer.y), rows - 1)))
        c.bg = SIMD4(0.55, 0.78, 1.0, isFocused ? 0.5 : 0.22)   // dimmer block when unfocused
        return c
    }

    // Main thread: resolve each descriptor's glyph against the atlas (rasterizing new
    // ones here, synchronized with the GPU read) and build the instance array.
    private func applyDescs(_ descs: [CellDesc]) {
        let hasSel = selStart != nil && selEnd != nil && selStart! != selEnd!
        instances = descs.map { d in
            var i = CellInstance()
            i.gridPos = d.gridPos; i.fg = d.fg; i.bg = d.bg
            if hasSel, selectionContains(Int(d.gridPos.x), Int(d.gridPos.y)) { i.bg = SIMD4(0.20, 0.35, 0.60, 1) }
            if let ch = d.ch {
                let g = atlas.glyph(for: ch, bold: d.style & 1 != 0, italic: d.style & 2 != 0, wide: d.wide)
                i.uvOrigin = g.origin; i.uvSize = g.size
            }
            i.underline = Float((d.style & 4 != 0 ? 1 : 0) | (d.style & 8 != 0 ? 2 : 0))   // bit0 underline, bit1 strike
            i.cellW = d.wide ? 2 : 1
            return i
        }
    }

    private func color(_ c: Attribute.Color, def: SIMD4<Float>) -> SIMD4<Float> {
        switch c {
        case .defaultColor, .defaultInvertedColor: return def
        case .trueColor(let r, let g, let b): return SIMD4(Float(r)/255, Float(g)/255, Float(b)/255, 1)
        case .ansi256(let code): return palette[Int(code)]
        }
    }

    // Re-tint the default fg/bg to the app theme and rebuild every cell (default-colored
    // cells cache the resolved color in the grid, so a theme flip needs a full refill).
    private var appliedThemeMode: ThemeMode?
    func applyTheme(_ mode: ThemeMode) {
        guard appliedThemeMode != mode else { return }
        appliedThemeMode = mode
        applyThemeColors(Self.themeColors(mode))
    }

    private func applyThemeColors(_ c: (fg: SIMD4<Float>, bg: SIMD4<Float>)) {
        clearColor = MTLClearColor(red: Double(c.bg.x), green: Double(c.bg.y), blue: Double(c.bg.z), alpha: 1)
        themeDelegate.fg = SwiftTerm.Color(red8: UInt16(c.fg.x * 255), green8: UInt16(c.fg.y * 255), blue8: UInt16(c.fg.z * 255))
        themeDelegate.bg = SwiftTerm.Color(red8: UInt16(c.bg.x * 255), green8: UInt16(c.bg.y * 255), blue8: UInt16(c.bg.z * 255))
        parseQueue.async { [weak self] in
            guard let self else { return }
            self.defFg = c.fg; self.defBg = c.bg
            self.gridCols = 0   // force full rebuild
            let descs = self.buildDescs()
            DispatchQueue.main.async { self.latestDescs = descs; self.needsPresent = true }
        }
    }

    static func themeColors(_ mode: ThemeMode) -> (fg: SIMD4<Float>, bg: SIMD4<Float>) {
        let pal: Palette = mode == .light ? Theme.light : (mode == .sepia ? Theme.sepia : Theme.dark)
        return (simd4(pal.fg), simd4(pal.bg))
    }

    private static func simd4(_ color: SwiftUI.Color) -> SIMD4<Float> {
        let ns = NSColor(color).usingColorSpace(.sRGB) ?? .black
        return SIMD4(Float(ns.redComponent), Float(ns.greenComponent), Float(ns.blueComponent), 1)
    }

    // Main thread only: copy the prepared instances into a reused GPU buffer and present.
    private func uploadAndRender() {
        let t0 = CACurrentMediaTime()
        guard metalLayer.drawableSize.width > 0, metalLayer.drawableSize.height > 0 else { return }
        let dt0 = CACurrentMediaTime()
        guard let drawable = metalLayer.nextDrawable() else { return }
        if (CACurrentMediaTime() - dt0) > 0.05 { PerfHUD.shared.tick("term:drawable-wait") }
        let n = instances.count
        if n > 0 {
            let need = MemoryLayout<CellInstance>.stride * n
            if instanceBuf == nil || instanceBuf!.length < need {
                instanceBuf = device.makeBuffer(length: max(need, need * 2), options: .storageModeShared)
            }
            instances.withUnsafeBytes { memcpy(instanceBuf!.contents(), $0.baseAddress!, need) }
        }
        uniforms.viewportPx = SIMD2(Float(metalLayer.drawableSize.width), Float(metalLayer.drawableSize.height))
        uniforms.cellPx = cellPx
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = clearColor
        pass.colorAttachments[0].storeAction = .store
        guard let cb = queue.makeCommandBuffer(), let enc = cb.makeRenderCommandEncoder(descriptor: pass) else { return }
        if let buf = instanceBuf, n > 0 {
            enc.setRenderPipelineState(pipeline)
            enc.setVertexBuffer(buf, offset: 0, index: 0)
            enc.setVertexBytes(&uniforms, length: MemoryLayout<TermUniforms>.stride, index: 1)
            enc.setFragmentTexture(atlas.texture, index: 0)
            enc.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6, instanceCount: n)
        }
        enc.endEncoding()
        cb.present(drawable)
        cb.commit()
        updateStats(mainMs: (CACurrentMediaTime() - t0) * 1000)
    }

    static let statsKey = "metalTermStats"

    private func updateStats(mainMs: Double) {
        let show = UserDefaults.standard.bool(forKey: Self.statsKey)
        if statsLabel.isHidden != !show { statsLabel.isHidden = !show }
        guard show else { return }
        frameCount += 1
        let now = CACurrentMediaTime()
        let dt = now - lastStatsAt
        guard dt >= 0.5 else { return }
        let fps = Double(frameCount) / dt
        statsLabel.stringValue = String(format: "main %.2f ms | parse %.2f ms | %.0f fps | %dx%d | %dKB | %d cells",
                                        mainMs, lastParseMs, fps, termCols, termRows, fedBytes / 1024, instances.count)
        frameCount = 0; lastStatsAt = now
    }

    private static func ansi256Palette() -> [SIMD4<Float>] {
        var p: [SIMD4<Float>] = []
        let base: [(Int, Int, Int)] = [
            (0,0,0),(205,0,0),(0,205,0),(205,205,0),(0,0,238),(205,0,205),(0,205,205),(229,229,229),
            (127,127,127),(255,0,0),(0,255,0),(255,255,0),(92,92,255),(255,0,255),(0,255,255),(255,255,255)]
        for (r,g,b) in base { p.append(SIMD4(Float(r)/255, Float(g)/255, Float(b)/255, 1)) }
        let steps = [0,95,135,175,215,255]
        for r in steps { for g in steps { for b in steps { p.append(SIMD4(Float(r)/255, Float(g)/255, Float(b)/255, 1)) } } }
        for i in 0..<24 { let v = 8 + i*10; p.append(SIMD4(Float(v)/255, Float(v)/255, Float(v)/255, 1)) }
        return p
    }

    static let shaderSource = """
    #include <metal_stdlib>
    using namespace metal;
    struct Cell { float2 gridPos; float2 uvOrigin; float2 uvSize; float4 fg; float4 bg; float underline; float cellW; };
    struct Uniforms { float2 viewportPx; float2 cellPx; };
    struct VOut { float4 pos [[position]]; float2 cellUV; float2 uvOrigin; float2 uvSize; float4 fg; float4 bg; float underline; };

    vertex VOut term_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                            const device Cell* cells [[buffer(0)]], constant Uniforms& u [[buffer(1)]]) {
        float2 quad[6] = { {0,0},{1,0},{0,1},{0,1},{1,0},{1,1} };
        float2 corner = quad[vid];
        Cell c = cells[iid];
        float2 originPx = c.gridPos * u.cellPx;
        float2 px = originPx + corner * float2(u.cellPx.x * c.cellW, u.cellPx.y);
        float2 ndc = (px / u.viewportPx) * 2.0 - 1.0;
        ndc.y = -ndc.y;
        VOut o;
        o.pos = float4(ndc, 0, 1);
        o.cellUV = corner;
        o.uvOrigin = c.uvOrigin; o.uvSize = c.uvSize; o.fg = c.fg; o.bg = c.bg; o.underline = c.underline;
        return o;
    }

    fragment float4 term_fragment(VOut in [[stage_in]], texture2d<float> atlas [[texture(0)]]) {
        constexpr sampler s(coord::normalized, filter::nearest);
        float coverage = 0.0;
        if (in.uvSize.x > 0.0) {
            float2 auv = in.uvOrigin + in.cellUV * in.uvSize;
            coverage = atlas.sample(s, auv).r;
        }
        int lf = int(in.underline + 0.5);
        if ((lf & 1) != 0 && in.cellUV.y > 0.88 && in.cellUV.y < 0.96) { coverage = 1.0; }
        if ((lf & 2) != 0 && in.cellUV.y > 0.46 && in.cellUV.y < 0.54) { coverage = 1.0; }
        return mix(in.bg, in.fg, coverage);
    }
    """
}

final class HeadlessTerminalDelegate: TerminalDelegate {
    var onChange: () -> Void = {}
    var fg = SwiftTerm.Color(red8: 0xd0, green8: 0xd0, blue8: 0xd0)
    var bg = SwiftTerm.Color(red8: 0x14, green8: 0x14, blue8: 0x17)
    func send(source: Terminal, data: ArraySlice<UInt8>) {}
    func scrolled(source: Terminal, yDisp: Int) { onChange() }
    func linefeed(source: Terminal) {}
    func bufferActivated(source: Terminal) { onChange() }
    func bell(source: Terminal) {}
    func sizeChanged(source: Terminal) { onChange() }
    func setTerminalTitle(source: Terminal, title: String) {}
    func setTerminalIconTitle(source: Terminal, title: String) {}
    func mouseModeChanged(source: Terminal) {}
    func showCursor(source: Terminal) {}
    func hideCursor(source: Terminal) {}
    func cursorStyleChanged(source: Terminal, newStyle: CursorStyle) {}
    func selectionChanged(source: Terminal) {}
    func isProcessTrusted(source: Terminal) -> Bool { true }
    func setForegroundColor(source: Terminal, color: SwiftTerm.Color) {}
    func setBackgroundColor(source: Terminal, color: SwiftTerm.Color) {}
    func setCursorColor(source: Terminal, color: SwiftTerm.Color?) {}
    func getColors(source: Terminal) -> (foreground: SwiftTerm.Color, background: SwiftTerm.Color) {
        (fg, bg)
    }
    func colorChanged(source: Terminal, idx: Int?) {}
    func hostCurrentDirectoryUpdated(source: Terminal) {}
    func hostCurrentDocumentUpdated(source: Terminal) {}
}
