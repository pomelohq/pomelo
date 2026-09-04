import Metal
import CoreText
import CoreGraphics
import simd

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
        let pos = CGPoint.zero
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
