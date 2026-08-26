import SwiftUI

// Updating the golden source: reset each main repo to its default branch and pull
// from origin. Shows per-repo progress + result instead of running silently.
struct UpdateMainSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.dismiss) private var dismiss
    let ws: Workspace

    private struct RepoResult: Decodable, Identifiable {
        var repo = ""; var branch = ""; var ok = false; var error = ""
        var id: String { repo }
    }
    private struct Payload: Decodable { var ok = false; var results: [RepoResult] = [] }

    @State private var running = true
    @State private var results: [RepoResult] = []

    private var repoNames: [String] { ws.repos.map(\.name) }
    private var allOK: Bool { !results.isEmpty && results.allSatisfy(\.ok) }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                Image(systemName: "arrow.triangle.2.circlepath").font(.system(size: 13)).foregroundStyle(Theme.accent)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Update main from origin").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text("Reset each repo to its default branch and pull the latest").font(.system(size: 11)).foregroundStyle(Theme.dim)
                }
                Spacer()
                Button { dismiss() } label: { Image(systemName: "xmark").font(.system(size: 12)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
            }
            .padding(.horizontal, 20).padding(.vertical, 14)
            Divider().overlay(Theme.borderSoft)

            ScrollView {
                VStack(spacing: 1) {
                    ForEach(running ? repoNames.map { RepoResult(repo: $0) } : results) { r in
                        row(r)
                    }
                }
                .padding(.vertical, 6)
            }

            Divider().overlay(Theme.borderSoft)
            HStack(spacing: 10) {
                if running {
                    ProgressView().controlSize(.small).scaleEffect(0.7)
                    Text("Pulling \(repoNames.count) repos…").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                } else {
                    Label(allOK ? "Up to date" : "Finished with errors",
                          systemImage: allOK ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                        .font(.system(size: 12)).foregroundStyle(allOK ? Theme.ok : Theme.danger)
                }
                Spacer()
                Button("Close") { dismiss() }.buttonStyle(.borderedProminent).tint(Theme.accent).disabled(running)
            }
            .padding(.horizontal, 20).padding(.vertical, 12)
        }
        .frame(width: 460, height: 360).background(Theme.bgSoft)
        .task { await run() }
    }

    private func row(_ r: RepoResult) -> some View {
        HStack(spacing: 9) {
            if running {
                ProgressView().controlSize(.mini)
            } else {
                Image(systemName: r.ok ? "checkmark.circle.fill" : "xmark.octagon.fill")
                    .font(.system(size: 12)).foregroundStyle(r.ok ? Theme.ok : Theme.danger)
            }
            Text(r.repo).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.fg)
            if !r.branch.isEmpty {
                Text(r.branch).font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
            }
            Spacer(minLength: 8)
            if !running && !r.ok && !r.error.isEmpty {
                Text(r.error).font(Theme.mono(10)).foregroundStyle(Theme.danger)
                    .lineLimit(1).truncationMode(.middle).frame(maxWidth: 180, alignment: .trailing)
                    .help(r.error)
            }
        }
        .padding(.horizontal, 18).padding(.vertical, 7)
    }

    private func run() async {
        running = true
        let branch = ws.branch
        let data = await Task.detached(priority: .userInitiated) { PomCore.shared.mainPull(branch: branch) }.value
        results = PomJSON.decode(Payload.self, from: data)?.results ?? []
        running = false
        await state.refreshWorkspaces()
    }
}
