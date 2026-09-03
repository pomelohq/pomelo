import SwiftUI
import AppKit

private struct JiraStatus: Decodable {
    var configured = false
    var site = ""
    var email = ""
    var token_set = false
    init() {}
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        configured = try c.decodeIfPresent(Bool.self, forKey: .configured) ?? false
        site = try c.decodeIfPresent(String.self, forKey: .site) ?? ""
        email = try c.decodeIfPresent(String.self, forKey: .email) ?? ""
        token_set = try c.decodeIfPresent(Bool.self, forKey: .token_set) ?? false
    }
    enum K: String, CodingKey { case configured, site, email, token_set }
}
private struct GithubStatus: Decodable {
    var installed = false
    var authed = false
    var account = ""
    init() {}
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        installed = try c.decodeIfPresent(Bool.self, forKey: .installed) ?? false
        authed = try c.decodeIfPresent(Bool.self, forKey: .authed) ?? false
        account = try c.decodeIfPresent(String.self, forKey: .account) ?? ""
    }
    enum K: String, CodingKey { case installed, authed, account }
}
private struct IntegrationsResponse: Decodable {
    var jira = JiraStatus()
    var github = GithubStatus()
    init() {}
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        jira = try c.decodeIfPresent(JiraStatus.self, forKey: .jira) ?? JiraStatus()
        github = try c.decodeIfPresent(GithubStatus.self, forKey: .github) ?? GithubStatus()
    }
    enum K: String, CodingKey { case jira, github }
}

struct IntegrationsSettings: View {
    @Environment(AppState.self) var state
    @State private var data = IntegrationsResponse()
    @State private var loading = true
    @State private var jiraTest = ""
    @State private var site = ""
    @State private var email = ""
    @State private var token = ""
    @State private var jiraSaved = false

    @State private var ghToken = ""
    @State private var ghSaved = false
    @State private var ghTest = ""


    var body: some View {
        @Bindable var state = state
        Form {
            Section {
                providerPicker("Provider", current: "Jira", soon: ["Linear", "GitHub Issues"])
                TextField("Site URL", text: $site, prompt: Text("https://acme.atlassian.net"))
                TextField("Account email", text: $email, prompt: Text("you@acme.com"))
                SecureField("API token", text: $token,
                            prompt: Text(data.jira.token_set ? "•••••• (saved — leave blank to keep)" : "paste your Jira API token"))
                HStack {
                    Button("Save") { saveJira() }.disabled(site.isEmpty || email.isEmpty)
                    Button("Test connection") { testJira() }.disabled(site.isEmpty || email.isEmpty)
                    if jiraSaved { Text("Saved").font(.system(size: 11)).foregroundStyle(Theme.ok) }
                    if !jiraTest.isEmpty { Text(jiraTest).font(.system(size: 11)).foregroundStyle(Theme.fgMuted) }
                }
                Toggle("Only show my tickets", isOn: $state.jiraOnlyMine)
            } header: { HStack(spacing: 6) { Text("Tracker · Jira"); if loading { Spinner(size: 10) } } } footer: {
                Text("Stored app-local: site/email in plain config, the token encrypted (AES-GCM). Nothing goes into the shareable pom.yml. \"Only show my tickets\" filters the New workspace ticket picker to issues assigned to you.")
            }

            Section {
                providerPicker("Provider", current: "GitHub", soon: ["GitLab", "Bitbucket"])
                SecureField("Fine-grained token", text: $ghToken,
                            prompt: Text(data.github.authed ? "•••••• (saved — leave blank to keep)" : "paste a GitHub PAT (Pull requests: read)"))
                HStack {
                    Button("Save") { saveGithub() }.disabled(ghToken.isEmpty)
                    Button("Test") { testGithub() }
                    if ghSaved { Text("Saved").font(.system(size: 11)).foregroundStyle(Theme.ok) }
                    if !ghTest.isEmpty { Text(ghTest).font(.system(size: 11)).foregroundStyle(Theme.fgMuted) }
                    Spacer()
                    statusDot(data.github.authed, data.github.authed ? "configured" : "no token")
                }
            } header: { Text("Forge · GitHub") } footer: {
                Text("Pomelo talks to GitHub directly (no `gh` CLI). A fine-grained PAT with Pull requests: read is enough — stored encrypted app-local (secret `github`), per session. Or export GH_TOKEN in your shell.")
            }

            // Secrets moved to the Session panel (Project ⌘⇧P › Secrets) — they are
            // per-session and referenced by the config shown there.

            SyncSection()

            Section {
                providerPicker("AI Agent", current: "Claude", soon: ["Codex", "Gemini"])
                providerPicker("Shell", current: "zsh", soon: ["bash", "fish"])
            } header: { Text("Machine · shared across sessions") } footer: {
                Text("Not per-session: the agent uses your machine's `claude` CLI login; services launch via your login zsh. Only one provider ships today — the picker shows what's supported; more are coming.")
            }
        }
        .formStyle(.grouped).scrollContentBackground(.hidden)
        .onAppear { refresh() }
    }

    private func providerRow(_ name: String, value: String, scope: String, ok: Bool? = nil) -> some View {
        LabeledContent {
            HStack(spacing: 6) {
                if let ok { Circle().fill(ok ? Theme.ok : Theme.dim).frame(width: 7, height: 7) }
                Text(value).foregroundStyle(Theme.fg)
                Text(scope).font(.system(size: 9.5, weight: .medium)).foregroundStyle(Theme.dim)
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(Theme.chip, in: Capsule())
            }
        } label: { Text(name) }
    }

    private func providerPicker(_ name: String, current: String, soon: [String]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(name).font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.fgMuted)
            HStack(spacing: 6) {
                providerChip(current, active: true, enabled: true)
                ForEach(soon, id: \.self) { s in providerChip(s, active: false, enabled: false) }
            }
        }
    }

    private func providerChip(_ label: String, active: Bool, enabled: Bool) -> some View {
        HStack(spacing: 5) {
            if active { Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)) }
            Text(label).font(.system(size: 12, weight: active ? .semibold : .regular))
            if !enabled {
                Text("soon").font(.system(size: 8.5, weight: .medium))
                    .padding(.horizontal, 4).padding(.vertical, 1)
                    .background(Theme.chip, in: Capsule()).foregroundStyle(Theme.dim)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 5)
        .foregroundStyle(active ? Theme.accent : Theme.dim)
        .background(active ? Theme.accent.opacity(0.14) : Theme.chip.opacity(0.5),
                    in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(active ? Theme.accent.opacity(0.5) : .clear, lineWidth: 1))
    }

    private func statusDot(_ ok: Bool, _ detail: String) -> some View {
        HStack(spacing: 6) {
            Circle().fill(ok ? Theme.ok : Theme.dim).frame(width: 8, height: 8)
            Text(detail).font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
        }
    }

    private func saveGithub() {
        ghSaved = false
        ghTest = ""
        let t = ghToken
        Task {
            await IntegrationsStore.setGithub(t)
            ghSaved = true
            ghToken = ""
            refresh()
        }
    }

    private func testGithub() {
        ghTest = "testing…"
        ghSaved = false
        let t = ghToken
        Task {
            let d = await IntegrationsStore.testGithub(t)
            struct R: Decodable {
                var ok = false
                var user: String?
                var error: String?
            }
            if let r = PomJSON.decode(R.self, from: d) {
                ghTest = r.ok ? "OK" + (r.user.map { " · \($0)" } ?? "") : ((r.error ?? "").isEmpty ? "failed" : r.error!)
            } else {
                ghTest = "failed"
            }
        }
    }

    private func refresh() {
        Task {
            let d = await IntegrationsStore.status()
            if let r = PomJSON.decode(IntegrationsResponse.self, from: d) {
                data = r; site = r.jira.site; email = r.jira.email
            }
            loading = false
        }
    }

    private func saveJira() {
        jiraSaved = false; jiraTest = ""
        let s = site, e = email, t = token
        Task {
            await IntegrationsStore.setJira(site: s, email: e, token: t)
            jiraSaved = true; token = ""; refresh()
        }
    }

    private func testJira() {
        jiraTest = "testing…"
        let (s, e, t) = (site, email, token)
        Task {
            let d = await IntegrationsStore.testJira(site: s, email: e, token: t)
            struct R: Decodable { var ok = false; var error: String? }
            if let r = PomJSON.decode(R.self, from: d) {
                jiraTest = r.ok ? "OK" : ((r.error ?? "").isEmpty ? "failed" : r.error!)
            } else { jiraTest = "failed" }
        }
    }

}
