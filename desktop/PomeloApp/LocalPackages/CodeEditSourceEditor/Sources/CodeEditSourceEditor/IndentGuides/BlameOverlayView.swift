//
//  BlameOverlayView.swift
//  CodeEditSourceEditor
//

import AppKit
import CodeEditTextView

/// Draws an inline git-blame annotation at the end of the line the caret is on (Zed-style). Lives in
/// the text view's document so it scrolls with the content; only the current line is annotated.
final class BlameOverlayView: NSView {
    weak var textView: TextView?
    var blameLines: [Int: String] = [:] {
        didSet { needsDisplay = true }
    }
    var textColor: NSColor = .tertiaryLabelColor {
        didSet { needsDisplay = true }
    }
    var font: NSFont = .monospacedSystemFont(ofSize: 11, weight: .regular)

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var isOpaque: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        guard let textView,
              let layoutManager = textView.layoutManager,
              let selection = textView.selectionManager?.textSelections.first,
              !blameLines.isEmpty else { return }
        guard let line = layoutManager.textLineForOffset(selection.range.location) else { return }
        guard let text = blameLines[line.index], !text.isEmpty else { return }

        // End of the line's content (before the newline), then pad past it.
        let endOffset = max(line.range.location, line.range.max - 1)
        guard let caret = layoutManager.rectForOffset(endOffset) else { return }

        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: textColor]
        let string = text as NSString
        let textSize = string.size(withAttributes: attrs)
        let x = caret.maxX + font.pointSize * 2
        let y = line.yPos + (line.height - textSize.height) / 2
        string.draw(at: NSPoint(x: x.rounded(), y: y.rounded()), withAttributes: attrs)
    }
}
