import SwiftUI

struct GitPanel: View {
    let workspace: Workspace
    @StateObject private var vm: GitViewModel

    init(workspace: Workspace) {
        self.workspace = workspace
        _vm = StateObject(wrappedValue: GitViewModel(branch: workspace.branch, isMain: workspace.isMain))
    }

    var body: some View {
        Group {
            if vm.loading {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if vm.totalChanges == 0 {
                emptyState
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 14) {
                        ForEach(vm.repos) { repo in
                            if !repo.isClean { RepoSection(vm: vm, repo: repo) }
                        }
                    }
                    .padding(12)
                }
            }
        }
        .overlay(alignment: .top) {
            if !vm.lastError.isEmpty {
                Text(vm.lastError)
                    .font(.system(size: 11)).foregroundStyle(Theme.danger)
                    .padding(.horizontal, 10).padding(.vertical, 5)
                    .background(Theme.dangerSoft, in: Capsule())
                    .padding(.top, 8)
            }
        }
        .task { await vm.load() }
        .refreshable { await vm.load() }
    }

    private var emptyState: some View {
        VStack(spacing: 6) {
            Image(systemName: "checkmark.seal").font(.system(size: 26)).foregroundStyle(Theme.ok)
            Text("No local changes").font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct RepoSection: View {
    @ObservedObject var vm: GitViewModel
    let repo: GitViewModel.RepoStatus

    private var message: Binding<String> {
        Binding(get: { vm.commitMessage[repo.repo] ?? "" },
                set: { vm.commitMessage[repo.repo] = $0 })
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            if !repo.staged.isEmpty {
                group(title: "Staged", changes: repo.staged, staged: true)
                commitBar
            }
            if !repo.unstaged.isEmpty {
                group(title: "Changes", changes: repo.unstaged, staged: false)
            }
        }
        .padding(10)
        .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.borderSoft, lineWidth: 1))
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "shippingbox").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            Text(repo.repo).font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.fg)
            Text(repo.branch).font(.system(size: 10)).foregroundStyle(Theme.fgMuted)
                .padding(.horizontal, 5).padding(.vertical, 1)
                .background(Theme.bg, in: Capsule())
            if repo.ahead > 0 {
                Label("\(repo.ahead)", systemImage: "arrow.up").font(.system(size: 10)).foregroundStyle(Theme.ok)
            }
            if repo.behind > 0 {
                Label("\(repo.behind)", systemImage: "arrow.down").font(.system(size: 10)).foregroundStyle(Theme.warn)
            }
            Spacer()
            if repo.ahead > 0 {
                Button { Task { await vm.push(repo.repo) } } label: {
                    Label("Push", systemImage: "arrow.up.circle").font(.system(size: 11))
                }
                .buttonStyle(.plain).foregroundStyle(Theme.accent).disabled(vm.busy)
            }
        }
    }

    @ViewBuilder private func group(title: String, changes: [GitViewModel.Change], staged: Bool) -> some View {
        HStack {
            Text("\(title) (\(changes.count))").font(.system(size: 10, weight: .medium)).foregroundStyle(Theme.fgMuted)
            Spacer()
            if staged {
                Button("Unstage all") { Task { await vm.unstage(repo.repo, changes.map(\.path)) } }
                    .buttonStyle(.plain).font(.system(size: 10)).foregroundStyle(Theme.accent).disabled(vm.busy)
            } else {
                Button("Stage all") { Task { await vm.stage(repo.repo, changes.map(\.path)) } }
                    .buttonStyle(.plain).font(.system(size: 10)).foregroundStyle(Theme.accent).disabled(vm.busy)
            }
        }
        .padding(.top, 2)
        ForEach(changes) { c in row(c, staged: staged) }
    }

    private func row(_ c: GitViewModel.Change, staged: Bool) -> some View {
        HStack(spacing: 8) {
            Text(c.badge)
                .font(.system(size: 10, weight: .bold, design: .monospaced))
                .foregroundStyle(badgeColor(c))
                .frame(width: 14)
            Text(c.path).font(.system(size: 11)).foregroundStyle(Theme.fg).lineLimit(1).truncationMode(.middle)
            Spacer()
            if staged {
                iconButton("minus.circle", "Unstage") { await vm.unstage(repo.repo, [c.path]) }
            } else {
                iconButton("arrow.uturn.backward.circle", "Discard", danger: true) { await vm.discard(repo.repo, [c.path]) }
                iconButton("plus.circle", "Stage") { await vm.stage(repo.repo, [c.path]) }
            }
        }
        .padding(.vertical, 1)
    }

    private var commitBar: some View {
        HStack(spacing: 6) {
            TextField("Commit message", text: message, axis: .vertical)
                .textFieldStyle(.plain).font(.system(size: 11))
                .padding(6).background(Theme.bg, in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Theme.borderSoft, lineWidth: 1))
            Button { Task { await vm.commit(repo.repo) } } label: {
                Label("Commit", systemImage: "checkmark").font(.system(size: 11))
            }
            .buttonStyle(.plain).foregroundStyle(Theme.ok)
            .disabled(vm.busy || (vm.commitMessage[repo.repo] ?? "").trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .padding(.vertical, 3)
    }

    private func iconButton(_ icon: String, _ help: String, danger: Bool = false, _ act: @escaping () async -> Void) -> some View {
        Button { Task { await act() } } label: {
            Image(systemName: icon).font(.system(size: 12))
                .foregroundStyle(danger ? Theme.danger : Theme.fgMuted)
        }
        .buttonStyle(.plain).disabled(vm.busy).help(help)
    }

    private func badgeColor(_ c: GitViewModel.Change) -> Color {
        if c.untracked { return Theme.ok }
        switch c.badge {
        case "D": return Theme.danger
        case "A": return Theme.ok
        default:  return Theme.accent
        }
    }
}
