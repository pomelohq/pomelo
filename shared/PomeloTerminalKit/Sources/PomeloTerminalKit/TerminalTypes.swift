import CoreGraphics
import simd
import SwiftTerm

// GPU instance: one textured quad per cell. Field layout mirrors the shader's `Cell`.
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
    var style: Int = 0    // 0 regular, |1 bold, |2 italic, |4 underline, |8 strike
    var wide = false      // CJK / emoji / nerd-font: the glyph spans two cells
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
