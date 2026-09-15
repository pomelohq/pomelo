import SwiftUI
import AppKit
import CodeEditSourceEditor
import CodeEditLanguages
import CodeEditTextView

// Editable source editor for the Files pane. Reuses CodeEditSourceEditor (already
// the engine behind the SQL editor) with tree-sitter language detected from the
// file name and the shared editor theme. Saving is the host's job (Cmd+S / Save).
struct FileEditor: View {
    @Binding var text: String
    let path: String
    var mode: ThemeMode
    var wrap: Bool = false
    var editable: Bool = true
    var fontSize: CGFloat = 12
    var changedLines: [Int: Int] = [:]
    var onRightClick: (NSPoint, NSView) -> Void = { _, _ in }
    @State private var state = SourceEditorState()

    var body: some View {
        SourceEditor(
            $text,
            language: CodeLanguage.detectLanguageFrom(url: URL(fileURLWithPath: path)),
            configuration: SourceEditorConfiguration(
                appearance: .init(
                    theme: SQLEditor.palette(mode),
                    useThemeBackground: true,
                    font: .monospacedSystemFont(ofSize: fontSize, weight: .regular),
                    lineHeightMultiple: 1.3,
                    wrapLines: wrap
                ),
                behavior: .init(isEditable: editable, indentOption: .spaces(count: 2)),
                peripherals: .init(showGutter: true, showMinimap: false, showIndentGuides: true)
            ),
            state: $state
        )
        .onRightClick(onRightClick)
        .changedLines(changedLines)
    }
}
