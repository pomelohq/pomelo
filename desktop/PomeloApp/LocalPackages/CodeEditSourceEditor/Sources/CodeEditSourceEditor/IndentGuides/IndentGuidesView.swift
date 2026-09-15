//
//  IndentGuidesView.swift
//  CodeEditSourceEditor
//

import AppKit
import CodeEditTextView

/// Draws Zed-style indentation guides as an overlay over the text view's document. Lives in the text view's
/// coordinate space (a subview of it) so it scrolls with the content and only ever redraws the exposed region,
/// which keeps the guides complete and gap-free unlike drawing inside the text view's own `draw`.
final class IndentGuidesView: NSView {
    struct Configuration: Equatable {
        var color: NSColor
        var stepWidth: CGFloat
        var spacesPerIndent: Int
        var lineWidth: CGFloat
    }

    weak var textView: TextView?
    var configuration: Configuration? {
        didSet {
            isHidden = configuration == nil
            needsDisplay = true
        }
    }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var isOpaque: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        guard let config = configuration,
              let textView,
              let layoutManager = textView.layoutManager,
              let textStorage = textView.textStorage,
              config.stepWidth > 0, config.spacesPerIndent > 0 else { return }

        let string = textStorage.string as NSString
        let originX = layoutManager.edgeInsets.left
        config.color.setFill()

        // Same iteration the gutter uses for line numbers: only laid-out lines in the range, stops at
        // content end. Robust through edits/folds where a manual y-stepping loop drops or over-draws guides.
        var lastDepth = 0
        for line in layoutManager.linesStartingAt(dirtyRect.minY, until: dirtyRect.maxY) {
            let (depth, blank) = indentDepth(of: line.range, in: string, spacesPerIndent: config.spacesPerIndent)
            let drawDepth: Int
            if blank {
                drawDepth = min(lastDepth, nextDepth(after: line, in: string, spacesPerIndent: config.spacesPerIndent))
            } else {
                drawDepth = depth
                lastDepth = depth
            }
            // Round top and bottom so consecutive segments meet exactly (no sub-pixel gaps).
            let y0 = line.yPos.rounded()
            let y1 = (line.yPos + line.height).rounded()
            // Start at level 1: like Zed, don't draw a guide at the first indent column.
            for level in 1..<max(drawDepth, 1) {
                let x = (originX + CGFloat(level) * config.stepWidth).rounded()
                NSRect(x: x, y: y0, width: config.lineWidth, height: y1 - y0).fill()
            }
        }
    }

    private func indentDepth(
        of range: NSRange,
        in string: NSString,
        spacesPerIndent: Int
    ) -> (depth: Int, blank: Bool) {
        var spaces = 0
        var tabs = 0
        var index = range.location
        let end = min(range.location + range.length, string.length)
        loop: while index < end {
            switch string.character(at: index) {
            case 0x20: spaces += 1
            case 0x09: tabs += 1
            case 0x0A, 0x0D: return (0, true)
            default: break loop
            }
            index += 1
        }
        if index >= end { return (0, true) }
        return (tabs + spaces / spacesPerIndent, false)
    }

    private func nextDepth(
        after line: TextLineStorage<TextLine>.TextLinePosition,
        in string: NSString,
        spacesPerIndent: Int
    ) -> Int {
        guard let layoutManager = textView?.layoutManager else { return 0 }
        var probeY = line.yPos + line.height
        var iterations = 0
        while iterations < 5_000, let next = layoutManager.textLineForPosition(probeY) {
            iterations += 1
            let (depth, blank) = indentDepth(of: next.range, in: string, spacesPerIndent: spacesPerIndent)
            if !blank { return depth }
            let advance = next.yPos + next.height
            if advance <= probeY { break }
            probeY = advance
        }
        return 0
    }
}
