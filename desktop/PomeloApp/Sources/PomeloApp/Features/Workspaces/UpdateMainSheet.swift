import SwiftUI

struct UpdateMainSheet: View {
    @Environment(AppState.self) var state
    @Environment(\.dismiss) private var dismiss
    let ws: Workspace

    // Optionals: Go omits `error` when ok, and omits `results`/`error` per branch;
    // Swift's synthesized Decodable throws on an absent non-optional key.
    private struct RepoResult: Decodable, Identifiable {
        var repo = ""; var branch = ""; var ok = false; var error: String?
        var id: String { repo }
    }
    private struct Payload: Decodable { var ok = false; var results: [RepoResult]?; var error: String? }

    @State private var running = true
    @State private var results: [RepoResult] = []
    @State private var ok = false
    @State private var rawError = ""

    private var repoNames: [String] { ws.repos.map(\.name) }
    private var allOK: Bool { ok && results.allSatisfy(\.ok) }

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
                    if !running && results.isEmpty {
                        VStack(spacing: 6) {
                            Text(rawError.isEmpty ? "No git repositories to update." : "Update failed")
                                .font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                            if !rawError.isEmpty {
                                Text(rawError).font(Theme.mono(10.5)).foregroundStyle(Theme.danger)
                                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                            }
                        }
                        .frame(maxWidth: .infinity).padding(20)
                    } else {
                        ForEach(running ? repoNames.map { RepoResult(repo: $0) } : results) { r in
                            row(r)
                        }
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
                    let good = allOK && !results.isEmpty
                    Label(good ? "Up to date" : (results.isEmpty ? "Nothing updated" : "Finished with errors"),
                          systemImage: good ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                        .font(.system(size: 12)).foregroundStyle(good ? Theme.ok : Theme.danger)
                }
                Spacer()
                Button("Close") { dismiss() }.buttonStyle(.borderedProminent).tint(Theme.accent).disabled(running)
            }
            .padding(.horizontal, 20).padding(.vertical, 12)
        }
        .frame(width: 460, height: 360).background(Theme.bgSoft)
        .task { await run() }
    }

    @State private var expanded: Set<String> = []

    private func row(_ r: RepoResult) -> some View {
        let err = (r.error ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let failed = !running && !r.ok && !err.isEmpty
        let open = expanded.contains(r.repo)
        return VStack(alignment: .leading, spacing: 6) {
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
                if failed {
                    Button { if open { expanded.remove(r.repo) } else { expanded.insert(r.repo) } } label: {
                        HStack(spacing: 3) {
                            Text(open ? "hide" : "why?").font(.system(size: 10.5, weight: .medium))
                            Image(systemName: open ? "chevron.up" : "chevron.down").font(.system(size: 8))
                        }.foregroundStyle(Theme.danger)
                    }.buttonStyle(.plain)
                }
            }
            if failed, open {
                Text(err).font(Theme.mono(10.5)).foregroundStyle(Theme.danger)
                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                    .padding(9).background(Theme.danger.opacity(0.1), in: RoundedRectangle(cornerRadius: 6))
            }
        }
        .padding(.horizontal, 18).padding(.vertical, 7)
    }

    private func run() async {
        running = true
        let branch = ws.branch
        let data = await WorkspaceOpsStore.mainPull(branch: branch)
        if let p = PomJSON.decode(Payload.self, from: data) {
            results = p.results ?? []; ok = p.ok; rawError = p.error ?? ""
        } else {
            rawError = String(data: data, encoding: .utf8) ?? "could not read result"
        }
        running = false
        await state.refreshWorkspaces()
    }
}
