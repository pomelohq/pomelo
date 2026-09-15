//
//  LineFoldRibbonView+Draw.swift
//  CodeEditSourceEditor
//
//  Created by Khan Winter on 5/8/25.
//

import AppKit
import CodeEditTextView

extension LineFoldRibbonView {
    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext,
              let model,
              let layoutManager = model.controller?.textView.layoutManager,
              let range = documentRange(covering: dirtyRect, layoutManager: layoutManager) else {
            return
        }

        context.saveGState()
        defer { context.restoreGState() }
        context.clip(to: dirtyRect)

        let foldsByLine = foldsByStartLine(in: range, layoutManager: layoutManager)

        for (lineNumber, fold) in foldsByLine {
            guard let line = layoutManager.textLineForIndex(lineNumber) else { continue }
            drawChevron(for: fold, on: line, model: model)
        }
    }

    /// Draws the disclosure chevron for one fold, centred on the line that opens it.
    ///
    /// An open fold only has a chevron while the pointer is over the gutter. A collapsed one always does, since it is
    /// the only sign left that anything is hidden there.
    private func drawChevron(
        for fold: FoldRange,
        on line: TextLineStorage<TextLine>.TextLinePosition,
        model: LineFoldModel
    ) {
        let isCollapsed = model.isCollapsed(fold)
        let isHovered = model.hoveredFold?.id == fold.id
        guard isCollapsed || isPointerInGutter else { return }

        let color = if isHovered {
            hoveredChevronColor
        } else if isCollapsed {
            collapsedChevronColor
        } else {
            chevronColor
        }

        guard let image = chevron(isCollapsed: isCollapsed, color: color) else { return }

        image.draw(
            in: CGRect(
                x: ((bounds.width - image.size.width) / 2).rounded(),
                y: (line.yPos + ((line.height - image.size.height) / 2)).rounded(),
                width: image.size.width,
                height: image.size.height
            )
        )
    }
}
