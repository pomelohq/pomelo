//
//  TextViewController+IndentGuides.swift
//  CodeEditSourceEditor
//

import AppKit
import CodeEditTextView

extension TextViewController {
    /// Pushes the current indent-guide configuration (derived from the theme, font and indent option)
    /// onto the text view, or clears it when disabled.
    func updateIndentGuides() {
        guard configuration.peripherals.showIndentGuides else {
            indentGuidesView?.configuration = nil
            return
        }
        let spacesPerIndent: Int
        switch indentOption {
        case .spaces(let count): spacesPerIndent = max(count, 1)
        case .tab: spacesPerIndent = max(tabWidth, 1)
        }
        indentGuidesView?.configuration = IndentGuidesView.Configuration(
            color: theme.invisibles.color.withAlphaComponent(0.55),
            stepWidth: font.charWidth * CGFloat(spacesPerIndent),
            spacesPerIndent: spacesPerIndent,
            lineWidth: 1
        )
    }

    /// Redraw guides after edits that can change indentation depth without resizing the document.
    func invalidateIndentGuides() {
        indentGuidesView?.needsDisplay = true
    }
}
