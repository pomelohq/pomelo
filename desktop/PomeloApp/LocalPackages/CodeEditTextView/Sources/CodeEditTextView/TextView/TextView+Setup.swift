//
//  TextView+Setup.swift
//  CodeEditTextView
//
//  Created by Khan Winter on 9/15/23.
//

import AppKit

extension TextView {
    func setUpLayoutManager(lineHeightMultiplier: CGFloat, wrapLines: Bool) -> TextLayoutManager {
        TextLayoutManager(
            textStorage: textStorage,
            lineHeightMultiplier: lineHeightMultiplier,
            wrapLines: wrapLines,
            textView: self,
            delegate: self
        )
    }

    func setUpSelectionManager() -> TextSelectionManager {
        TextSelectionManager(
            layoutManager: layoutManager,
            textStorage: textStorage,
            textView: self,
            delegate: self
        )
    }

    func setUpScrollListeners(scrollView: NSScrollView) {
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(scrollViewWillStartScroll),
            name: NSScrollView.willStartLiveScrollNotification,
            object: scrollView
        )

        NotificationCenter.default.addObserver(
            self,
            selector: #selector(scrollViewDidEndScroll),
            name: NSScrollView.didEndLiveScrollNotification,
            object: scrollView
        )

        NotificationCenter.default.addObserver(
            forName: NSView.boundsDidChangeNotification,
            object: scrollView.contentView,
            queue: .main
        ) { [weak self] _ in
            self?.updatedViewport(self?.visibleRect ?? .zero)
        }

        NotificationCenter.default.addObserver(
            forName: NSView.frameDidChangeNotification,
            object: scrollView.contentView,
            queue: .main
        ) { [weak self] _ in
            self?.updatedViewport(self?.visibleRect ?? .zero)
        }

        // Host broadcasts this when a pane-divider drag ends, so the (frozen) text view relayouts once.
        NotificationCenter.default.addObserver(
            forName: .ceForceRelayout, object: nil, queue: .main
        ) { [weak self] _ in
            guard let self else { return }
            self.updateFrameIfNeeded()
            self.layoutManager.layoutLines()
            self.needsDisplay = true
        }
    }

    @objc func scrollViewWillStartScroll() {
        if #available(macOS 14.0, *) {
            inputContext?.textInputClientWillStartScrollingOrZooming()
        }
    }

    @objc func scrollViewDidEndScroll() {
        if #available(macOS 14.0, *) {
            inputContext?.textInputClientDidEndScrollingOrZooming()
        }
    }
}
