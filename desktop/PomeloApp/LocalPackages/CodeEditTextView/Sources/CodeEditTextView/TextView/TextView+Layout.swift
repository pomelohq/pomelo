//
//  TextView+Layout.swift
//  CodeEditTextView
//
//  Created by Khan Winter on 6/15/24.
//

import Foundation

/// Set by the host while a pane divider is being dragged. Text views then skip their per-frame line relayout so the
/// resize is composited by Core Animation (GPU) instead of re-laying-out lines on the CPU each frame; the host clears
/// it on drag end and the final frame change relayouts once.
public nonisolated(unsafe) var CETextViewSuppressLayout = false

public extension Notification.Name {
    /// Post after a pane-divider drag ends to make frozen text views relayout once.
    static let ceForceRelayout = Notification.Name("CEForceRelayout")
}

extension TextView {
    override public func layout() {
        isPerformingLayout = true
        defer { isPerformingLayout = false }
        super.layout()
        guard !CETextViewSuppressLayout else { return }
        layoutManager.layoutLines()
        selectionManager.updateSelectionViews(skipTimerReset: true)
    }

    open override class var isCompatibleWithResponsiveScrolling: Bool {
        true
    }

    open override func prepareContent(in rect: NSRect) {
        needsLayout = true
        super.prepareContent(in: rect)
    }

    /// `open` so a subclass in another module can paint its own decorations under the text. Everything drawn here
    /// lands beneath the line fragment views, so an override that paints before `super` sits under the caret line
    /// highlight and the selection as well.
    open override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if isSelectable {
            selectionManager.drawSelections(in: dirtyRect)
        }
        emphasisManager?.updateLayerBackgrounds()
    }

    override open var isFlipped: Bool {
        true
    }

    override public var visibleRect: NSRect {
        if let scrollView {
            var rect = scrollView.documentVisibleRect
            rect.origin.y += scrollView.contentInsets.top
            return rect.pixelAligned
        } else {
            return super.visibleRect
        }
    }

    public var visibleTextRange: NSRange? {
        let minY = max(visibleRect.minY, 0)
        let maxY = min(visibleRect.maxY, layoutManager.estimatedHeight())
        guard let minYLine = layoutManager.textLineForPosition(minY),
              let maxYLine = layoutManager.textLineForPosition(maxY) else {
            return nil
        }
        return NSRange(
            location: minYLine.range.location,
            length: (maxYLine.range.location - minYLine.range.location) + maxYLine.range.length
        )
    }

    public func updatedViewport(_ newRect: CGRect) {
        // Frozen during a pane-divider drag: skip the per-frame line relayout triggered by the scroll view's bounds
        // change (updateFrameIfNeeded returns false while frozen, which would otherwise fall through to layoutLines).
        if CETextViewSuppressLayout { return }
        if !updateFrameIfNeeded() {
            layoutManager.layoutLines()
        }
        inputContext?.invalidateCharacterCoordinates()
    }

    /// Updates the view's frame if needed depending on wrapping lines, a new maximum width, or changed available size.
    /// - Returns: Whether or not the view was updated.
    @discardableResult
    public func updateFrameIfNeeded() -> Bool {
        // Frozen during a pane-divider drag (see CETextViewSuppressLayout) so resizing doesn't recompute the frame.
        if CETextViewSuppressLayout { return false }
        // Never mutate the frame or request layout from inside the window's layout pass. Doing so re-enters the
        // display cycle and on macOS 26+ aborts the window ("more Layout Window passes than views"). Coalesce to the
        // next runloop turn, where a single follow-up pass converges once the frame fits the content.
        if isPerformingLayout {
            if !frameUpdateScheduled {
                frameUpdateScheduled = true
                DispatchQueue.main.async { [weak self] in
                    self?.frameUpdateScheduled = false
                    self?.updateFrameIfNeeded()
                }
            }
            return false
        }

        var availableSize = scrollView?.contentSize ?? .zero
        availableSize.height -= (scrollView?.contentInsets.top ?? 0) + (scrollView?.contentInsets.bottom ?? 0)

        let extraHeight = availableSize.height * overscrollAmount
        let newHeight = max(layoutManager.estimatedHeight() + extraHeight, availableSize.height, 0)
        let newWidth = layoutManager.estimatedWidth()

        var didUpdate = false

        if newHeight >= availableSize.height && frame.size.height != newHeight {
            frame.size.height = newHeight
            // No need to update layout after height adjustment
        }

        if wrapLines && frame.size.width != availableSize.width {
            frame.size.width = availableSize.width
            didUpdate = true
        } else if !wrapLines && frame.size.width != max(newWidth, availableSize.width) {
            frame.size.width = max(newWidth, availableSize.width)
            didUpdate = true
        }

        if didUpdate {
            needsLayout = true
            needsDisplay = true
            layoutManager.setNeedsLayout()
        }

        if isSelectable {
            selectionManager?.updateSelectionViews()
        }

        return didUpdate
    }
}
