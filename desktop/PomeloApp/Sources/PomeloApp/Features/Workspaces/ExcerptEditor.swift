import AppKit
import SwiftUI

// Editable, syntax-highlighted code view for one project-search excerpt (Zed multibuffer style). Uses a fixed line
// height so a sibling SwiftUI line-number gutter stays aligned. Reports edits so the host can write them back to the
// file. Non-scrolling: the view sizes to its content and lives inside the results ScrollView.
struct ExcerptEditor: NSViewRepresentable {
    var lines: [NSAttributedString]
    var lineHeight: CGFloat
    var fontSize: CGFloat
    var textColor: NSColor
    var onEdit: ([String]) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onEdit: onEdit) }

    func makeNSView(context: Context) -> NSTextView {
        let tv = ExcerptTextView()
        tv.isEditable = true
        tv.isSelectable = true
        tv.isRichText = false
        tv.allowsUndo = true
        tv.drawsBackground = false
        tv.textContainerInset = .zero
        tv.textContainer?.lineFragmentPadding = 0
        tv.textContainer?.widthTracksTextView = true
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = false
        tv.autoresizingMask = [.width]
        tv.delegate = context.coordinator
        tv.insertionPointColor = textColor
        context.coordinator.fontSize = fontSize
        context.coordinator.lineHeight = lineHeight
        context.coordinator.apply(to: tv, lines: lines, force: true)
        return tv
    }

    func updateNSView(_ tv: NSTextView, context: Context) {
        context.coordinator.fontSize = fontSize
        context.coordinator.lineHeight = lineHeight
        context.coordinator.onEdit = onEdit
        // Only reset text when the source changed and the user isn't mid-edit, to avoid stealing the caret.
        context.coordinator.apply(to: tv, lines: lines, force: false)
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var onEdit: ([String]) -> Void
        var fontSize: CGFloat = 12
        var lineHeight: CGFloat = 15
        private var lastApplied: String = ""
        private var editing = false

        init(onEdit: @escaping ([String]) -> Void) { self.onEdit = onEdit }

        func apply(to tv: NSTextView, lines: [NSAttributedString], force: Bool) {
            let joined = lines.map { $0.string }.joined(separator: "\n")
            if !force && (editing || joined == lastApplied) { return }
            lastApplied = joined
            let out = NSMutableAttributedString()
            let para = NSMutableParagraphStyle()
            para.minimumLineHeight = lineHeight
            para.maximumLineHeight = lineHeight
            for (i, ln) in lines.enumerated() {
                if i > 0 { out.append(NSAttributedString(string: "\n")) }
                out.append(ln)
            }
            out.addAttributes([
                .font: NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular),
                .paragraphStyle: para
            ], range: NSRange(location: 0, length: out.length))
            tv.textStorage?.setAttributedString(out)
        }

        func textDidBeginEditing(_ notification: Notification) { editing = true }
        func textDidEndEditing(_ notification: Notification) { editing = false }

        func textDidChange(_ notification: Notification) {
            guard let tv = notification.object as? NSTextView else { return }
            let text = tv.string
            lastApplied = text
            onEdit(text.components(separatedBy: "\n"))
        }
    }
}

// Sizes itself to fit its text so the excerpt grows/shrinks with edits inside the SwiftUI ScrollView.
final class ExcerptTextView: NSTextView {
    override var intrinsicContentSize: NSSize {
        guard let lm = layoutManager, let tc = textContainer else { return super.intrinsicContentSize }
        lm.ensureLayout(for: tc)
        let h = lm.usedRect(for: tc).height
        return NSSize(width: NSView.noIntrinsicMetric, height: ceil(h))
    }

    override func didChangeText() {
        super.didChangeText()
        invalidateIntrinsicContentSize()
    }
}
