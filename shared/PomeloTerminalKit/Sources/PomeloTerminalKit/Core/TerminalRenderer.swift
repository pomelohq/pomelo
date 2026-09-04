import Foundation
import Metal
import QuartzCore
import CoreText
import CoreGraphics
import simd
import SwiftTerm

// GPU terminal renderer. Reuses SwiftTerm's headless `Terminal` (VT parser + grid
// buffer) and replaces the drawing: each unique glyph is rasterized once into a
// texture atlas and the screen is drawn as one instanced pass of textured quads
// (the Alacritty/Ghostty model). Platform-neutral: owns a CAMetalLayer + CADisplayLink
// and exposes semantic input (feed, scroll, select) that a thin NSView/UIView host
// drives from its native events. Colors and backing scale are supplied by the host.
public final class TerminalRenderer: NSObject {
    private static let sharedDevice = MTLCreateSystemDefaultDevice()!
    private static let sharedPipeline: MTLRenderPipelineState = {
        let lib = try! sharedDevice.makeLibrary(source: TerminalRenderer.shaderSource, options: nil)
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

    private let device = TerminalRenderer.sharedDevice
    private let queue: MTLCommandQueue
    public let metalLayer = CAMetalLayer()
    private var pipeline: MTLRenderPipelineState!
    private var atlas: GlyphAtlas!
    private var instances: [CellInstance] = []
    private var instanceBuf: MTLBuffer?
    private var uniforms = TermUniforms()
    private var cellPx = SIMD2<Float>(8, 16)
    private let parseQueue = DispatchQueue(label: "pom.term.parse")
    private var latestDescs: [CellDesc]?
    private var needsPresent = false
    private var grid: [CellDesc] = []
    private var gridCols = 0, gridRows = 0

    public let terminal: Terminal
    private let palette: [SIMD4<Float>]
    public private(set) var termCols = 80, termRows = 24
    public var onResize: ((Int, Int) -> Void)?
    // Emits a formatted stats line when stats are enabled; the host decides how to show it.
    public var onStats: ((String) -> Void)?
    public var statsEnabled = false

    private var defFg = SIMD4<Float>(0.85, 0.85, 0.85, 1)
    private var defBg = SIMD4<Float>(0.08, 0.08, 0.09, 1)
    private var clearColor = MTLClearColor(red: 0.08, green: 0.08, blue: 0.09, alpha: 1)
    private let themeDelegate: HeadlessTerminalDelegate

    private var lastParseMs: Double = 0
    private var frameCount = 0
    private var lastStatsAt = CACurrentMediaTime()
    private var fedBytes = 0

    private var fontSize: CGFloat = 12
    private var fontFamily = ""
    private var scale: CGFloat = 2

    public override init() {
        queue = device.makeCommandQueue()!
        palette = Self.ansi256Palette()
        let delegate = HeadlessTerminalDelegate()
        themeDelegate = delegate
        terminal = Terminal(delegate: delegate)
        super.init()
        delegate.onChange = {}   // feed() drives the rebuild + present
        metalLayer.device = device
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = true
        setupFontAndAtlas()
        pipeline = Self.sharedPipeline
    }

    // MARK: - Host-facing configuration

    public var cellPixelWidth: CGFloat { CGFloat(cellPx.x) }
    public var cellPixelHeight: CGFloat { CGFloat(cellPx.y) }
    public var viewportYDisp: Int { viewYDisp }
    public var mouseModeOn: Bool { if case .off = terminal.mouseMode { return false } else { return true } }
    public var hasSelection: Bool { selStart != nil && selEnd != nil && selStart! != selEnd! }

    public func setFont(family: String, size: CGFloat) {
        guard family != fontFamily || abs(size - fontSize) > 0.1 else { return }
        fontFamily = family; fontSize = size
        setupFontAndAtlas()
        lastYDisp = -1
    }

    public func setColors(fg: SIMD4<Float>, bg: SIMD4<Float>) {
        guard fg != defFg || bg != defBg else { return }
        clearColor = MTLClearColor(red: Double(bg.x), green: Double(bg.y), blue: Double(bg.z), alpha: 1)
        themeDelegate.fg = SwiftTerm.Color(red8: UInt16(fg.x * 255), green8: UInt16(fg.y * 255), blue8: UInt16(fg.z * 255))
        themeDelegate.bg = SwiftTerm.Color(red8: UInt16(bg.x * 255), green8: UInt16(bg.y * 255), blue8: UInt16(bg.z * 255))
        parseQueue.async { [weak self] in
            guard let self else { return }
            self.defFg = fg; self.defBg = bg
            self.gridCols = 0   // force full rebuild
            let descs = self.buildDescs()
            DispatchQueue.main.async { self.latestDescs = descs; self.needsPresent = true }
        }
    }

    public func setFocused(_ focused: Bool) {
        guard isFocused != focused else { return }
        isFocused = focused
        parseQueue.async { [weak self] in
            guard let self else { return }
            let descs = self.buildDescs()
            DispatchQueue.main.async { self.latestDescs = descs; self.needsPresent = true }
        }
    }

    // Point size of the hosting view + its backing scale. Sets drawable size, recomputes
    // the grid, resizes the engine, and fires onResize when the cell count changes.
    public func updateGeometry(pointSize: CGSize, scale newScale: CGFloat) {
        let scaleChanged = abs(newScale - scale) > 0.01
        scale = newScale
        if scaleChanged { setupFontAndAtlas() }
        metalLayer.contentsScale = scale
        let px = CGSize(width: pointSize.width * scale, height: pointSize.height * scale)
        guard px.width > 0, px.height > 0 else { return }
        metalLayer.drawableSize = px
        let cols = max(1, Int(px.width / CGFloat(cellPx.x)))
        let rows = max(1, Int(px.height / CGFloat(cellPx.y)))
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

    // The host owns the CADisplayLink (macOS creates it via NSView.displayLink(target:),
    // iOS via CADisplayLink(target:)) and calls this each vsync. Present the latest frame.
    @objc public func renderTick() {
        guard needsPresent, let descs = latestDescs else { return }
        needsPresent = false
        applyDescs(descs)
        uploadAndRender()
    }

    // Host calls on detach (view left the window) to stop autoscroll timers.
    public func teardown() { setAutoscroll(0) }

    // MARK: - Feed

    public func feed(_ bytes: [UInt8]) {
        fedBytes += bytes.count
        parseQueue.async { [weak self] in
            guard let self else { return }
            let before = self.terminal.buffer.yDisp
            self.terminal.feed(byteArray: bytes)
            // SwiftTerm snaps the viewport to the live bottom on output. If the user has
            // scrolled up, restore their position so streaming output doesn't yank them down.
            self.scrollBottomYDisp = self.terminal.buffer.yDisp
            if self.userScrolledUp { self.terminal.buffer.yDisp = min(before, self.scrollBottomYDisp) }
            let t0 = CACurrentMediaTime()
            let descs = self.buildDescs()
            let yd = self.terminal.buffer.yDisp
            self.lastParseMs = (CACurrentMediaTime() - t0) * 1000
            DispatchQueue.main.async { self.latestDescs = descs; self.viewYDisp = yd; self.needsPresent = true }
        }
    }

    public func feed(_ bytes: ArraySlice<UInt8>) { feed(Array(bytes)) }

    // MARK: - Scrollback

    private var scrollBottomYDisp = 0
    private var userScrolledUp = false
    private var lastYDisp = -1
    private var isFocused = false
    private var viewYDisp = 0
    private var selStart: (col: Int, row: Int)?
    private var selEnd: (col: Int, row: Int)?

    // Move the viewport through scrollback. lines > 0 shows older lines (smaller yDisp).
    public func scrollLines(_ lines: Int) {
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

    // MARK: - Selection (host passes screen-relative col/row; anchors are absolute)

    public func selectWord(col: Int, screenRow: Int) {
        let (lo, hi) = wordRange(col: col, row: screenRow)
        selStart = (lo, screenRow + viewYDisp); selEnd = (hi, screenRow + viewYDisp)
        needsPresent = true
    }

    public func selectLine(screenRow: Int) {
        selStart = (0, screenRow + viewYDisp); selEnd = (max(0, termCols - 1), screenRow + viewYDisp)
        needsPresent = true
    }

    public func beginSelect(col: Int, screenRow: Int) {
        let c = (col, screenRow + viewYDisp); selStart = c; selEnd = c; needsPresent = true
    }

    public func extendSelect(col: Int, screenRow: Int) {
        selEnd = (col, screenRow + viewYDisp); needsPresent = true
    }

    // Clears the selection if it never grew past its anchor (a plain click).
    public func endSelectClearIfEmpty() {
        if let s = selStart, let e = selEnd, s == e { selStart = nil; selEnd = nil; needsPresent = true }
    }

    public func clearSelection() { selStart = nil; selEnd = nil; needsPresent = true }

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

    // Drag-past-edge autoscroll while selecting. dir < 0 = older (up), dir > 0 = newer.
    private var autoscrollTimer: Timer?
    private var autoscrollDir = 0
    private var lastDragCol = 0

    public func beginAutoscroll(dir: Int, col: Int) { lastDragCol = col; setAutoscroll(dir) }
    public func endAutoscroll() { setAutoscroll(0) }

    private func setAutoscroll(_ dir: Int) {
        if dir == 0 { autoscrollTimer?.invalidate(); autoscrollTimer = nil; autoscrollDir = 0; return }
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

    // Extract the selected text (parseQueue work), returned on the main thread.
    public func selectionText(_ completion: @escaping (String) -> Void) {
        guard let s = selStart, let e = selEnd, s != e else { completion(""); return }
        parseQueue.async { [weak self] in
            guard let self else { completion(""); return }
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
            DispatchQueue.main.async { completion(out) }
        }
    }

    private func le(_ p: (Int, Int), _ q: (Int, Int)) -> Bool { p.1 < q.1 || (p.1 == q.1 && p.0 <= q.0) }

    private func selectionContains(_ col: Int, _ row: Int) -> Bool {
        guard let s = selStart, let e = selEnd, s != e else { return false }
        let p = (col, row + viewYDisp)   // screen row -> absolute buffer row
        let a = le(s, e) ? s : e, b = le(s, e) ? e : s
        return le(a, p) && le(p, b)
    }

    // MARK: - Font / atlas

    private func setupFontAndAtlas() {
        let font = TerminalFonts.monoFont(fontSize, family: fontFamily)
        let advance = advanceWidth(font)
        let ascent = CTFontGetAscent(font), descent = CTFontGetDescent(font), leading = CTFontGetLeading(font)
        let s = Float(scale)
        let cw = Int((advance * CGFloat(s)).rounded(.up))
        let ch = Int(((ascent + descent + leading) * CGFloat(s)).rounded(.up))
        cellPx = SIMD2(Float(cw), Float(ch))
        let scaledFont = TerminalFonts.monoFont(fontSize * CGFloat(s), family: fontFamily)
        atlas = Self.sharedAtlas(device: device, font: scaledFont, cellW: cw, cellH: ch)
        for u in 32...126 { if let sc = UnicodeScalar(u) { _ = atlas.glyph(for: Character(sc)) } }
    }

    private static var atlasCache: [String: GlyphAtlas] = [:]
    private static func sharedAtlas(device: MTLDevice, font: CTFont, cellW: Int, cellH: Int) -> GlyphAtlas {
        let name = (CTFontCopyPostScriptName(font) as String)
        let key = "\(name)|\(cellW)x\(cellH)"
        if let a = atlasCache[key] { return a }
        let a = GlyphAtlas(device: device, font: font, cellW: cellW, cellH: cellH)!
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

    // MARK: - Grid build (parseQueue)

    private func buildDescs() -> [CellDesc] {
        let cols = terminal.cols, rows = terminal.rows
        let yd = terminal.buffer.yDisp
        let sizeChanged = gridCols != cols || gridRows != rows || grid.count != cols * rows
        if sizeChanged {
            grid = Array(repeating: CellDesc(), count: cols * rows)
            gridCols = cols; gridRows = rows
        }
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
                    // NOT cd.getCharacter(): astral scalars (Material Design nerd icons,
                    // U+F0000+) are stored via the terminal's grapheme-index map; the
                    // CharData-only getter returns a space for them.
                    let ch = terminal.getCharacter(for: cd)
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
        c.bg = SIMD4(0.55, 0.78, 1.0, isFocused ? 0.5 : 0.22)
        return c
    }

    // MARK: - Instance build + present (main)

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
            i.underline = Float((d.style & 4 != 0 ? 1 : 0) | (d.style & 8 != 0 ? 2 : 0))
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

    private func uploadAndRender() {
        let t0 = CACurrentMediaTime()
        guard metalLayer.drawableSize.width > 0, metalLayer.drawableSize.height > 0 else { return }
        guard let drawable = metalLayer.nextDrawable() else { return }
        let n = instances.count
        if n > 0 {
            let need = MemoryLayout<CellInstance>.stride * n
            if instanceBuf == nil || instanceBuf!.length < need {
                instanceBuf = device.makeBuffer(length: max(need, need * 2), options: .storageModeShared)
            }
            instances.withUnsafeBytes { ptr in _ = memcpy(instanceBuf!.contents(), ptr.baseAddress!, need) }
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

    private func updateStats(mainMs: Double) {
        guard statsEnabled, let onStats else { return }
        frameCount += 1
        let now = CACurrentMediaTime()
        let dt = now - lastStatsAt
        guard dt >= 0.5 else { return }
        let fps = Double(frameCount) / dt
        let line = String(format: "main %.2f ms | parse %.2f ms | %.0f fps | %dx%d | %dKB | %d cells",
                          mainMs, lastParseMs, fps, termCols, termRows, fedBytes / 1024, instances.count)
        frameCount = 0; lastStatsAt = now
        onStats(line)
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
