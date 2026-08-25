import SwiftUI
import AppKit

extension AppState {
    func switchSession(_ name: String) {
        Task {
            // A freshly created session isn't in the cached list yet — reload before failing.
            if !sessions.contains(where: { $0.name == name }) { await loadSessions() }
            guard let dir = sessions.first(where: { $0.name == name })?.path, !dir.isEmpty else {
                switchError = "Can't switch to “\(name)”: session unavailable"
                return
            }
            let hasCfg = ["pom.yml"].contains {
                FileManager.default.fileExists(atPath: (dir as NSString).appendingPathComponent($0))
            }
            guard hasCfg else {
                switchError = "Can't switch to “\(name)”: no pom.yml in \(dir)"
                return
            }
            // Re-boot into the session's project — the whole FFI/ptyhost/session state is
            // tied to the booted config, so an in-place cfg swap can't switch it cleanly.
            bootProject(dir)
        }
    }

    func deleteSession(_ name: String) {
        Task {
            _ = await Task.detached { PomCore.shared.sessionDelete(name: name, purge: false) }.value
            await loadSessions()
        }
    }
}

struct CreateSessionSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var defaultBranch = "main"
    @State private var repos: [RepoRow] = []
    @State private var repoURL = ""
    @State private var busy = false
    @State private var status = ""

    struct RepoRow: Identifiable { let id = UUID(); var path: String; var alias: String }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 11) {
                Image(systemName: "plus.rectangle.on.folder.fill")
                    .font(.system(size: 20)).foregroundStyle(Theme.accent)
                VStack(alignment: .leading, spacing: 2) {
                    Text("New session").font(.system(size: 16, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text("Clone repos, scaffold a session, and switch to it.")
                        .font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
                }
                Spacer()
            }

            HStack(alignment: .top, spacing: 12) {
                labeled("Session name") {
                    TextField("myproject", text: $name).textFieldStyle(.plain)
                        .font(.system(size: 13)).padding(.horizontal, 10).padding(.vertical, 7)
                        .background(Theme.bg, in: RoundedRectangle(cornerRadius: 7))
                        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.borderSoft))
                }
                labeled("Default branch") {
                    TextField("main", text: $defaultBranch).textFieldStyle(.plain)
                        .font(Theme.mono(12.5)).padding(.horizontal, 10).padding(.vertical, 7)
                        .background(Theme.bg, in: RoundedRectangle(cornerRadius: 7))
                        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.borderSoft))
                        .frame(width: 130)
                }
                .fixedSize()
            }

            VStack(alignment: .leading, spacing: 7) {
                HStack {
                    Text("REPOSITORIES").font(.system(size: 10, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
                    if !repos.isEmpty {
                        Text("\(repos.count)").font(.system(size: 10, weight: .bold)).foregroundStyle(Theme.fgMuted)
                            .padding(.horizontal, 5).padding(.vertical, 1).background(Theme.sel, in: Capsule())
                    }
                    Spacer()
                    Button { pickRepo() } label: {
                        Label("Add folder…", systemImage: "folder.badge.plus").font(.system(size: 12))
                    }.buttonStyle(.plain).foregroundStyle(Theme.accent)
                }
                HStack(spacing: 6) {
                    Image(systemName: "link").font(.system(size: 11)).foregroundStyle(Theme.dim)
                    TextField("or paste a git URL (git@host:org/repo.git · https://…)", text: $repoURL)
                        .textFieldStyle(.plain).font(Theme.mono(11.5))
                        .onSubmit { addURL() }
                    Button("Add") { addURL() }
                        .font(.system(size: 11.5)).buttonStyle(.plain).foregroundStyle(Theme.accent)
                        .disabled(repoURL.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .padding(.horizontal, 10).padding(.vertical, 6)
                .background(Theme.bg, in: RoundedRectangle(cornerRadius: 7))
                .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.borderSoft))
                ScrollView {
                    VStack(spacing: 5) {
                        ForEach($repos) { $r in repoRow($r) }
                    }.padding(6).frame(maxWidth: .infinity)
                }
                .frame(maxWidth: .infinity, minHeight: 172, maxHeight: 172)
                .background(Theme.bg, in: RoundedRectangle(cornerRadius: 9))
                .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.borderSoft))
                .overlay {
                    if repos.isEmpty {
                        VStack(spacing: 8) {
                            Image(systemName: "folder.badge.plus").font(.system(size: 26)).foregroundStyle(Theme.dim)
                            Text("Add at least one local git repo").font(.system(size: 12)).foregroundStyle(Theme.dim)
                            Text("Each becomes a repo in the session; edit its alias inline.")
                                .font(.system(size: 10.5)).foregroundStyle(Theme.dim.opacity(0.7))
                        }
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .padding(.horizontal, 24)
                        .allowsHitTesting(false)
                    }
                }
            }

            HStack(alignment: .top, spacing: 8) {
                Image(systemName: "sparkles").font(.system(size: 11)).foregroundStyle(Theme.accent)
                Text("After creating, an onboarding agent reads each repo and writes a runnable pom.yml — frameworks, services, shared infra, and env — looping until config_doctor is clean.")
                    .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            }
            .padding(10).background(Theme.accent.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))

            if !status.isEmpty {
                Text(status).font(.system(size: 11)).foregroundStyle(status.hasPrefix("✓") ? Theme.ok : Theme.danger)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction).disabled(busy)
                Button(busy ? "Creating…" : "Create") { create() }.buttonStyle(.borderedProminent).tint(Theme.accent)
                    .disabled(busy || name.isEmpty || repos.isEmpty)
            }
        }
        .padding(20).frame(width: 580)
    }

    private func labeled<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 10, weight: .semibold)).kerning(0.4).foregroundStyle(Theme.muted)
            content()
        }
    }

    private func repoRow(_ r: Binding<RepoRow>) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "shippingbox.fill").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            TextField("alias", text: r.alias).textFieldStyle(.plain)
                .font(Theme.mono(12)).foregroundStyle(Theme.fg)
                .padding(.horizontal, 7).padding(.vertical, 3)
                .background(Theme.sel, in: RoundedRectangle(cornerRadius: 5))
                .frame(width: 118)
            Text(r.wrappedValue.path).font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
                .lineLimit(1).truncationMode(.middle)
            Spacer()
            Button(role: .destructive) { repos.removeAll { $0.id == r.wrappedValue.id } } label: {
                Image(systemName: "xmark.circle.fill").font(.system(size: 12))
            }.buttonStyle(.plain).foregroundStyle(Theme.dim)
        }
        .padding(.horizontal, 8).padding(.vertical, 5)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 6))
    }

    private func addURL() {
        let u = repoURL.trimmingCharacters(in: .whitespaces)
        guard !u.isEmpty else { return }
        repos.append(RepoRow(path: u, alias: aliasFromURL(u)))
        repoURL = ""
    }

    private func aliasFromURL(_ u: String) -> String {
        var s = u.hasSuffix(".git") ? String(u.dropLast(4)) : u
        s = s.hasSuffix("/") ? String(s.dropLast()) : s
        if let i = s.lastIndex(where: { $0 == "/" || $0 == ":" }) { s = String(s[s.index(after: i)...]) }
        return s.isEmpty ? "repo" : s
    }

    private func pickRepo() {
        let p = NSOpenPanel(); p.canChooseDirectories = true; p.canChooseFiles = false; p.allowsMultipleSelection = true
        p.begin { resp in
            guard resp == .OK else { return }
            for url in p.urls {
                repos.append(RepoRow(path: url.path, alias: url.lastPathComponent))
            }
        }
    }

    private func create() {
        busy = true; status = ""
        let reposJSON = repos.map { "{\"path\":\"\($0.path)\",\"alias\":\"\($0.alias)\"}" }.joined(separator: ",")
        let body = "{\"name\":\"\(name)\",\"default_branch\":\"\(defaultBranch)\",\"repos\":[\(reposJSON)]}"
        Task {
            let d = await Task.detached { PomCore.shared.sessionCreate(json: body) }.value
            let ok = String(decoding: d, as: UTF8.self).contains("\"ok\":true") || String(decoding: d, as: UTF8.self).contains(name)
            busy = false
            if ok {
                status = "✓ created"; state.switchSession(name)
                try? await Task.sleep(nanoseconds: 800_000_000)
                let b = defaultBranch
                dismiss()
                state.onboardBranch = b
            }
            else { status = String(decoding: d, as: UTF8.self).prefix(120).description }
        }
    }
}
