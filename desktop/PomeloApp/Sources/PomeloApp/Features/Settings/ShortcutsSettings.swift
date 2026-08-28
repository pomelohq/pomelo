import SwiftUI

struct ShortcutsSettings: View {
    private struct Shortcut: Identifiable { let keys, label: String; var id: String { keys + label } }

    private let global: [Shortcut] = [
        .init(keys: "⌘N", label: "New workspace"),
        .init(keys: "⇧⌘N", label: "New session"),
        .init(keys: "⇧⌘S", label: "Shared services"),
        .init(keys: "⇧⌘P", label: "Session — config editor + ENV"),
        .init(keys: "⇧⌘0", label: "Activity Monitor (all workspaces)"),
        .init(keys: "⇧⌘T", label: "Cycle theme"),
        .init(keys: "⌘,", label: "Settings"),
    ]
    private let workspace: [Shortcut] = [
        .init(keys: "⌘T", label: "New terminal"),
        .init(keys: "⌘J", label: "Toggle terminal drawer"),
        .init(keys: "⌘B", label: "Toggle sidebar"),
        .init(keys: "⌘E", label: "Open in editor"),
        .init(keys: "⌘0", label: "Activity (this workspace)"),
        .init(keys: "⌘1", label: "Services"),
        .init(keys: "⌘2", label: "Pull requests"),
        .init(keys: "⌘3", label: "Jira"),
        .init(keys: "⌘4", label: "Database"),
        .init(keys: "⌘I", label: "Agent"),
        .init(keys: "⌘W", label: "Close focused terminal tab"),
    ]

    var body: some View {
        Form {
            Section("Global") { ForEach(global) { row($0) } }
            Section {
                ForEach(workspace) { row($0) }
            } header: { Text("Workspace") } footer: {
                Text("Apply to the selected workspace. Pull requests, Jira and Claude are hidden on main.")
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
    }

    private func row(_ s: Shortcut) -> some View {
        LabeledContent(s.label) {
            Card(cornerRadius: 5, background: Theme.panel3) {
                Text(s.keys).font(Theme.mono(12)).foregroundStyle(Theme.fgMuted)
                    .padding(.horizontal, 7).padding(.vertical, 3)
            }
        }
    }
}
