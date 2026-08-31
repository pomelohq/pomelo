import XCTest
import AppKit
@testable import PomeloApp

// The global key monitors stand down while the user is typing. Read-only text
// views (code diff, PR narrative) still become first responder when clicked, so
// matching on NSTextView alone killed every app shortcut after clicking a diff.
@MainActor
final class TextEntryFocusTests: XCTestCase {
    func testEditableTextViewIsTextEntry() {
        let tv = NSTextView()
        tv.isEditable = true
        XCTAssertTrue(AppState.isTextEntry(tv))
    }

    // The regression: clicking a diff or the review narrative must not disable ⌘1/⌘I.
    func testReadOnlySelectableTextViewIsNotTextEntry() {
        let tv = NSTextView()
        tv.isEditable = false
        tv.isSelectable = true
        XCTAssertFalse(AppState.isTextEntry(tv))
    }

    // Matches CodeTextView.configureReadOnly(), which backs the PRs Files tab.
    func testCodeTextViewConfiguredReadOnlyIsNotTextEntry() {
        let tv = CodeTextView()
        tv.configureReadOnly(inset: .zero)
        XCTAssertFalse(AppState.isTextEntry(tv))
    }

    // CodeEditSourceEditor's view is an NSView, never an NSTextView.
    func testNonTextResponderIsNotTextEntry() {
        XCTAssertFalse(AppState.isTextEntry(NSView()))
        XCTAssertFalse(AppState.isTextEntry(nil))
    }

    // A field editor reports editable even though the NSTextField owns it.
    func testFieldEditorCountsAsTextEntry() {
        let field = NSTextField(string: "")
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 200, height: 60),
                              styleMask: [.titled], backing: .buffered, defer: true)
        window.contentView?.addSubview(field)
        guard let editor = window.fieldEditor(true, for: field) as? NSTextView else {
            return XCTFail("expected an NSTextView field editor")
        }
        XCTAssertTrue(AppState.isTextEntry(editor))
    }
}
