import SwiftUI

// Read-only view of one file's uncommitted diff, shown as an editor tab when a file is picked in the git panel.
struct GitDiffTab: View {
    let workspace: Workspace
    let entry: WorkspaceFileEntry

    @State private var files: [DiffFile]?
    @State private var selFile: String?
    @State private var treeVisible = false
    @State private var splitDiff = false

    var body: some View {
        DiffFilesView(files: files?.filter { $0.path == entry.path },
                      selFile: $selFile, filesTreeVisible: $treeVisible, splitDiff: $splitDiff,
                      loadingLabel: "loading diff...", emptyLabel: "No uncommitted changes")
            .task(id: entry.id) { await load() }
    }

    private func load() async {
        selFile = entry.path
        let branch = workspace.branch, isMain = workspace.isMain, repo = entry.repo
        let data = await Task.detached(priority: .userInitiated) {
            PRStore.localDiff(branch: branch, repo: repo, isMain: isMain)
        }.value
        files = PomJSON.decode([DiffFile].self, from: data) ?? []
        selFile = entry.path
    }
}
