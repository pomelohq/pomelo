import SwiftUI

// Workspace-level Cmd+P palette: fuzzy-jump to any file. Lives above the panes so it
// works regardless of which pane is active; choosing a file opens it in the Files pane.
struct FileQuickOpen: View {
    let entries: [WorkspaceFileEntry]
    let onChoose: (WorkspaceFileEntry) -> Void
    let onClose: () -> Void

    @State private var query = ""
    @State private var index = 0
    @FocusState private var focused: Bool

    private var results: [WorkspaceFileEntry] {
        let files = entries.filter { !$0.isDir }
        let q = query.trimmingCharacters(in: .whitespaces)
        if q.isEmpty { return Array(files.prefix(60)) }
        return files.compactMap { e -> (WorkspaceFileEntry, Int)? in
            let full = e.repo.isEmpty ? e.path : e.repo + "/" + e.path
            guard let s = Fuzzy.score(q, full) else { return nil }
            let name = Fuzzy.score(q, (e.path as NSString).lastPathComponent) ?? 0
            return (e, s + name)
        }
        .sorted { $0.1 > $1.1 }.prefix(60).map { $0.0 }
    }

    var body: some View {
        ZStack(alignment: .top) {
            Color.black.opacity(0.2).ignoresSafeArea().onTapGesture { onClose() }
            VStack(spacing: 0) {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                    TextField("Go to file...", text: $query)
                        .textFieldStyle(.plain).font(.system(size: 14)).foregroundStyle(Theme.fg)
                        .focused($focused)
                        .onChange(of: query) { _ in index = 0 }
                        .onSubmit { choose() }
                }
                .padding(.horizontal, 12).padding(.vertical, 10)
                Divider().overlay(Theme.borderSoft)
                if results.isEmpty {
                    Text("No files").font(.system(size: 12)).foregroundStyle(Theme.dim)
                        .frame(maxWidth: .infinity).padding(.vertical, 22)
                } else {
                    ScrollViewReader { proxy in
                        ScrollView {
                            LazyVStack(spacing: 0) {
                                ForEach(Array(results.enumerated()), id: \.element.id) { i, e in
                                    row(e, active: i == index).id(i)
                                        .contentShape(Rectangle())
                                        .onTapGesture { onChoose(e) }
                                }
                            }
                        }
                        .frame(maxHeight: 340)
                        .onChange(of: index) { proxy.scrollTo($0, anchor: .center) }
                    }
                }
            }
            .frame(width: 540)
            .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.border, lineWidth: 1))
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .shadow(color: .black.opacity(0.3), radius: 20, y: 8)
            .padding(.top, 72)
        }
        .onKeyPress(.downArrow) { index = min(index + 1, max(0, results.count - 1)); return .handled }
        .onKeyPress(.upArrow) { index = max(index - 1, 0); return .handled }
        .onKeyPress(.escape) { onClose(); return .handled }
        .onAppear { focused = true }
    }

    private func choose() {
        let r = results
        if index >= 0, index < r.count { onChoose(r[index]) }
    }

    private func row(_ e: WorkspaceFileEntry, active: Bool) -> some View {
        let mat = MaterialIcon.file(e.path).map { "mi-" + $0 }
        let dir = (e.path as NSString).deletingLastPathComponent
        return HStack(spacing: 8) {
            Group {
                if let mat { Image(mat, bundle: .module).resizable().aspectRatio(contentMode: .fit) }
                else { Image(systemName: "doc").foregroundStyle(Theme.fgMuted) }
            }.frame(width: 15, height: 15)
            Text((e.path as NSString).lastPathComponent).font(.system(size: 12.5)).foregroundStyle(Theme.fg)
            Text(e.repo.isEmpty ? dir : e.repo + (dir.isEmpty ? "" : "/" + dir))
                .font(.system(size: 11)).foregroundStyle(Theme.dim).lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(active ? Theme.sel : .clear)
    }
}
