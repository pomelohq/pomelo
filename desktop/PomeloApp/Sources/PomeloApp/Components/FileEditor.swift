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
    @State private var state = SourceEditorState()

    var body: some View {
        SourceEditor(
            $text,
            language: CodeLanguage.detectLanguageFrom(url: URL(fileURLWithPath: path)),
            configuration: SourceEditorConfiguration(
                appearance: .init(
                    theme: SQLEditor.palette(mode),
                    useThemeBackground: true,
                    font: .monospacedSystemFont(ofSize: 12, weight: .regular),
                    wrapLines: wrap
                ),
                behavior: .init(indentOption: .spaces(count: 2)),
                peripherals: .init(showGutter: true, showMinimap: false)
            ),
            state: $state
        )
    }
}
