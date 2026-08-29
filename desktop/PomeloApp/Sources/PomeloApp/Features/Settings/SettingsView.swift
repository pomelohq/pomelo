import SwiftUI
import AppKit
import UniformTypeIdentifiers
import ServiceManagement

enum SettingsGroup { case app, project }

enum SettingsSection: String, CaseIterable, Identifiable {
    case general, notifications, shortcuts, network, diagnostics, integrations
    var id: String { rawValue }
    var group: SettingsGroup {
        switch self {
        case .general, .notifications, .shortcuts, .network, .diagnostics: return .app
        case .integrations: return .project
        }
    }
    var title: String {
        switch self {
        case .general: return "General"
        case .notifications: return "Notifications"
        case .shortcuts: return "Shortcuts"
        case .network: return "Network"
        case .diagnostics: return "Diagnostics"
        case .integrations: return "Integrations"
        }
    }
    var icon: String {
        switch self {
        case .general: return "gearshape"
        case .notifications: return "bell"
        case .shortcuts: return "keyboard"
        case .network: return "network"
        case .diagnostics: return "ladybug"
        case .integrations: return "puzzlepiece.extension"
        }
    }
    var ready: Bool { true }
    var soonHint: String { "" }
}

struct SettingsView: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    @Environment(\.dismiss) private var dismiss
    @State private var section: SettingsSection = .general

    var body: some View {
        HStack(spacing: 0) {
            nav
            Divider().overlay(Theme.borderSoft)
            detail
        }
        .background(Theme.bg)
        .ignoresSafeArea(.container, edges: .top)
        .frame(width: 880, height: 600)
        .background(WindowConfigurator())
    }

    private var nav: some View {
        VStack(alignment: .leading, spacing: 2) {
            Color.clear.frame(height: 30)
            navGroup("APP", .app)
            navGroup("SESSION · \(PomCore.shared.session.isEmpty ? "—" : PomCore.shared.session)", .project)
            Spacer()
        }
        .frame(width: 200)
        .background(Theme.bgSoft)
    }

    @ViewBuilder private func navGroup(_ label: String, _ group: SettingsGroup) -> some View {
        Text(label).font(.system(size: 9.5, weight: .semibold)).kerning(0.6).foregroundStyle(Theme.muted)
            .lineLimit(1).truncationMode(.tail)
            .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, 3)
        ForEach(SettingsSection.allCases.filter { $0.group == group }) { s in
            Button { section = s } label: {
                HStack(spacing: 9) {
                    Image(systemName: s.icon).font(.system(size: 12)).frame(width: 16)
                    Text(s.title).font(.system(size: 12.5, weight: section == s ? .semibold : .regular))
                    Spacer(minLength: 0)
                    if !s.ready { Text("soon").font(.system(size: 9.5)).foregroundStyle(Theme.dim) }
                }
                .foregroundStyle(section == s ? Theme.fg : Theme.fgMuted)
                .padding(.horizontal, 10).padding(.vertical, 6)
                .background(section == s ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 6))
            }
            .buttonStyle(.plain).padding(.horizontal, 6)
        }
    }

    @ViewBuilder private var detail: some View {
        VStack(spacing: 0) {
            Text(section.title).font(.system(size: 18, weight: .bold)).foregroundStyle(Theme.fg)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 24).frame(height: 44)
            switch section {
            case .general: GeneralSettings()
            case .notifications: NotificationsSettings()
            case .shortcuts: ShortcutsSettings()
            case .network: NetworkSettings()
            case .diagnostics: DiagnosticsSettings()
            case .integrations: IntegrationsSettings()
            }
        }
        .frame(maxWidth: .infinity)
    }
}

private struct GeneralSettings: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    @ObservedObject private var codeDisplay = CodeDisplayManager.shared
    @State private var editors: [String] = []
    @State private var version = ""
    @State private var releasesURL = "https://github.com/pomelohq/pomelo/releases/latest"
    @State private var autoCheck = AppUpdater.shared.automaticChecks
    @State private var startAtLogin = SMAppService.mainApp.status == .enabled

    private func setLoginItem(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() }
            else { try SMAppService.mainApp.unregister() }
        } catch {
            startAtLogin = SMAppService.mainApp.status == .enabled
        }
    }

    var body: some View {
        Form {
            Section {
                Picker("Theme", selection: $theme.mode) {
                    ForEach(ThemeMode.allCases, id: \.self) { Text($0.rawValue.capitalized).tag($0) }
                }
                .pickerStyle(.segmented)
                LabeledContent {
                    Picker("", selection: $codeDisplay.wrapMode) {
                        ForEach(CodeWrapMode.allCases, id: \.self) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented).labelsHidden()
                } label: {
                    HStack(spacing: 5) {
                        Text("Read-only long lines of code")
                        HelpHint("Applies to diffs and file previews — anywhere code is shown but not edited. Wrap keeps a long line on screen by breaking it across rows; Scroll keeps it on one row, which is what holds the side-by-side diff aligned line-for-line.")
                    }
                }
                LabeledContent {
                    Picker("", selection: $codeDisplay.defaultSplit) {
                        Text("Unified").tag(false)
                        Text("Split").tag(true)
                    }
                    .pickerStyle(.segmented).labelsHidden()
                } label: {
                    Text("Default diff view")
                }
            } header: { Text("Appearance") }
            Section {
                Picker("Open with (⌘E)", selection: $state.editorPref) {
                    Text("Auto-detect").tag("")
                    ForEach(editors, id: \.self) { Text($0).tag($0) }
                }
            } header: { Text("Editor") } footer: {
                Text("Which GUI editor ⌘E launches. Auto tries VS Code, Cursor, Zed, Windsurf, Sublime.")
            }
            Section {
                Toggle("Start at login", isOn: $startAtLogin)
                    .onChange(of: startAtLogin) { setLoginItem(startAtLogin) }
            } header: { Text("Startup") } footer: {
                Text("Launch Pomelo automatically when you log in.")
            }
            Section {
                Button { state.openSetup() } label: { Label("Run setup guide", systemImage: "sparkles") }
            } header: { Text("Onboarding") } footer: {
                Text("Step through the essentials — notifications and the Claude MCP.")
            }
            Section {
                LabeledContent("Version") {
                    Text(version.isEmpty ? "…" : version).monospaced().foregroundStyle(Theme.fgMuted)
                }
                Toggle("Check for updates automatically", isOn: $autoCheck)
                    .onChange(of: autoCheck) { AppUpdater.shared.automaticChecks = autoCheck }
                HStack {
                    Button("Check for Updates…") {
                        AppUpdater.shared.checkForUpdates()
                    }.buttonStyle(.borderedProminent).tint(Theme.accent)
                    Button("View release") {
                        if let u = URL(string: releasesURL) { NSWorkspace.shared.open(u) }
                    }
                }
            } header: { Text("Updates") } footer: {
                Text("Pomelo updates itself via Sparkle: “Check for Updates” downloads the new release, verifies its signature, installs it, and relaunches — no manual download.")
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .task { await load() }
    }

    private func load() async {
        struct Editors: Decodable { var installed: [String] = [] }
        let ed = await SettingsStore.editors()
        if let d = PomJSON.decode(Editors.self, from: ed) { editors = d.installed }
        struct V: Decodable { var version = ""; var releases_url = "" }
        let d = await SettingsStore.version()
        if let v = PomJSON.decode(V.self, from: d) {
            version = v.version
            if !v.releases_url.isEmpty { releasesURL = v.releases_url }
        }
    }
}

private struct CfgWebhook: Decodable {
    var configured = false; var enabled = false; var listen_port = 0
    init() {}
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        configured = try c.decodeIfPresent(Bool.self, forKey: .configured) ?? false
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? false
        listen_port = try c.decodeIfPresent(Int.self, forKey: .listen_port) ?? 0
    }
    enum K: String, CodingKey { case configured, enabled, listen_port }
}
private struct CfgNetwork: Decodable {
    var bind_ip = "127.0.0.1"; var domain = "localhost"; var proxy_port = 0; var proxy_url = ""
    var webhook = CfgWebhook()
    var proxy_running = false; var webhook_running = false
    init() {}
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        bind_ip = try c.decodeIfPresent(String.self, forKey: .bind_ip) ?? "127.0.0.1"
        domain = try c.decodeIfPresent(String.self, forKey: .domain) ?? "localhost"
        proxy_port = try c.decodeIfPresent(Int.self, forKey: .proxy_port) ?? 0
        proxy_url = try c.decodeIfPresent(String.self, forKey: .proxy_url) ?? ""
        webhook = try c.decodeIfPresent(CfgWebhook.self, forKey: .webhook) ?? CfgWebhook()
        proxy_running = try c.decodeIfPresent(Bool.self, forKey: .proxy_running) ?? false
        webhook_running = try c.decodeIfPresent(Bool.self, forKey: .webhook_running) ?? false
    }
    enum K: String, CodingKey { case bind_ip, domain, proxy_port, proxy_url, webhook, proxy_running, webhook_running }
}

private struct NetworkSettings: View {
    @State private var net = CfgNetwork()
    @State private var proxyPort = 8767
    @State private var webhookPort = 8766
    @State private var origProxy = 8767
    @State private var origWebhook = 8766
    @State private var busy = false

    private var dirty: Bool { proxyPort != origProxy || webhookPort != origWebhook }

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    LabeledContent("Status") { statusChip(net.proxy_running) }
                    LabeledContent("From the frontend") {
                        Text("/_pom_dev/<repo>/<service>").monospaced().foregroundStyle(Theme.accent).textSelection(.enabled)
                    }
                    LabeledContent("Proxy port") {
                        TextField("", value: $proxyPort, format: .number.grouping(.never))
                            .frame(width: 80).multilineTextAlignment(.trailing).monospaced()
                    }
                    LabeledContent("Direct / tools") {
                        Text("<service>.<repo>.<ticket|branch>.localhost:\(String(proxyPort))")
                            .monospaced().textSelection(.enabled).lineLimit(1).truncationMode(.middle)
                    }
                    LabeledContent("Bind address") { Text(net.bind_ip).monospaced() }
                } header: { Text("Reverse proxy") } footer: {
                    Text("Point your frontend's backend base URL at /_pom_dev/<repo>/<service> — same-origin (cookies like prod), auto-mapped, no config. Switching environment retargets it; the frontend URL never changes.")
                }
                Section {
                    LabeledContent("Status") { statusChip(net.webhook_running) }
                    LabeledContent("Listen port") {
                        TextField("", value: $webhookPort, format: .number.grouping(.never))
                            .frame(width: 80).multilineTextAlignment(.trailing).monospaced()
                    }
                    LabeledContent("Public URL") {
                        Text("localhost:\(String(webhookPort))/<repo>/<service>")
                            .monospaced().textSelection(.enabled).lineLimit(1).truncationMode(.middle)
                    }
                } header: { Text("Webhook fan-out") } footer: {
                    Text("Point an external webhook (Stripe, Twilio, …) at /<repo>/<service> on this port — it fans out to every workspace running that service. Auto-mapped — no config.")
                }
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)
            Divider().overlay(Theme.borderSoft)
            HStack {
                if !net.proxy_running || !net.webhook_running {
                    Button("Start servers") { Task { await startServers() } }
                        .buttonStyle(.bordered).disabled(busy)
                }
                Spacer()
                if busy { Spinner() }
                Button("Apply and Restart") { Task { await applyRestart() } }
                    .buttonStyle(.borderedProminent).tint(Theme.accent)
                    .disabled(!dirty || busy)
            }
            .padding(.horizontal, 20).padding(.vertical, 12)
        }
        .task { await load() }
    }

    private func statusChip(_ running: Bool) -> some View {
        HStack(spacing: 5) {
            Circle().fill(running ? Theme.ok : Theme.danger).frame(width: 7, height: 7)
            Text(running ? "Running" : "Stopped").font(.system(size: 11, weight: .medium))
                .foregroundStyle(running ? Theme.ok : Theme.danger)
        }
    }

    private func startServers() async {
        busy = true
        _ = await SettingsStore.networkStart()
        await load()
        busy = false
    }

    private func load() async {
        let d = await SettingsStore.network()
        if let r = PomJSON.decode(CfgNetwork.self, from: d) {
            net = r
            if r.proxy_port > 0 { proxyPort = r.proxy_port; origProxy = r.proxy_port }
            if r.webhook.listen_port > 0 { webhookPort = r.webhook.listen_port; origWebhook = r.webhook.listen_port }
        }
    }

    private func applyRestart() async {
        busy = true
        let pp = proxyPort, wp = webhookPort
        await SettingsStore.setPorts(proxyPort: pp, webhookPort: wp)
        let url = Bundle.main.bundleURL
        let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/bin/open"); p.arguments = ["-n", url.path]
        try? p.run()
        NSApp.terminate(nil)
    }
}


private struct CfgSvcRef: Decodable, Hashable {
    var repo = ""; var alias = ""; var services: [String] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
        alias = try c.decodeIfPresent(String.self, forKey: .alias) ?? ""
        services = try c.decodeIfPresent([String].self, forKey: .services) ?? []
    }
    enum K: String, CodingKey { case repo, alias, services }
}
private struct CfgEnvPair: Decodable, Identifiable {
    var key = ""; var value = ""; var source = ""; var secret = false; var id: String { key }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        value = try c.decodeIfPresent(String.self, forKey: .value) ?? ""
        source = try c.decodeIfPresent(String.self, forKey: .source) ?? ""
        secret = try c.decodeIfPresent(Bool.self, forKey: .secret) ?? false
    }
    enum K: String, CodingKey { case key, value, source, secret }
}
private struct CfgExplain: Decodable {
    var repo = ""; var alias = ""; var service = ""; var cmd = ""; var port = 0
    var databases: [String: String] = [:]; var env: [CfgEnvPair] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
        alias = try c.decodeIfPresent(String.self, forKey: .alias) ?? ""
        service = try c.decodeIfPresent(String.self, forKey: .service) ?? ""
        cmd = try c.decodeIfPresent(String.self, forKey: .cmd) ?? ""
        port = try c.decodeIfPresent(Int.self, forKey: .port) ?? 0
        databases = try c.decodeIfPresent([String: String].self, forKey: .databases) ?? [:]
        env = try c.decodeIfPresent([CfgEnvPair].self, forKey: .env) ?? []
    }
    enum K: String, CodingKey { case repo, alias, service, cmd, port, databases, env }
}
private struct CfgExplainResp: Decodable {
    var repos: [CfgSvcRef] = []; var explain: CfgExplain?
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        repos = try c.decodeIfPresent([CfgSvcRef].self, forKey: .repos) ?? []
        explain = try c.decodeIfPresent(CfgExplain.self, forKey: .explain)
    }
    enum K: String, CodingKey { case repos, explain }
}

struct EnvInspector: View {
    @EnvironmentObject var state: AppState
    @State private var repos: [CfgSvcRef] = []
    @State private var profiles: [String] = []
    @State private var repo = ""
    @State private var svc = ""
    @State private var branch = ""
    @State private var profile = ""   // "" = local
    @State private var explain: CfgExplain?
    @State private var loading = false
    @State private var revealedEnv: Set<String> = []
    @State private var errText = ""

    private var services: [String] { repos.first(where: { $0.repo == repo })?.services ?? [] }
    private var branches: [String] {
        let bs = state.workspaces.map(\.branch)
        return bs.isEmpty ? [branch].filter { !$0.isEmpty } : bs
    }

    var body: some View {
        VStack(spacing: 0) {
            pickers
            Divider().overlay(Theme.borderSoft)
            if let e = explain { table(e) }
            else if !errText.isEmpty {
                Text(errText).font(Theme.mono(11)).foregroundStyle(Theme.danger)
                    .padding(16).frame(maxWidth: .infinity, alignment: .leading)
            } else { placeholder }
        }
    }

    private var pickers: some View {
        HStack(spacing: 14) {
            picker("Repo", $repo, repos.map(\.repo), labels: Dictionary(uniqueKeysWithValues: repos.map { ($0.repo, $0.alias) }))
            picker("Service", $svc, services)
            picker("Branch", $branch, branches, width: 190)
            picker("Profile", $profile, [""] + profiles, labels: ["": "local"])
            Spacer(minLength: 0)
            if loading { Spinner() }
        }
        .padding(.horizontal, 20).padding(.vertical, 12)
        .task { await boot() }
        .onChange(of: repo) { svc = services.first ?? ""; Task { await reload() } }
        .onChange(of: svc) { Task { await reload() } }
        .onChange(of: branch) { Task { await reload() } }
        .onChange(of: profile) { Task { await reload() } }
    }

    private func picker(_ title: String, _ sel: Binding<String>, _ opts: [String], labels: [String: String] = [:], width: CGFloat? = nil) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title.uppercased()).font(.system(size: 9.5, weight: .semibold)).kerning(0.5).foregroundStyle(Theme.dim)
            Picker("", selection: sel) {
                ForEach(opts, id: \.self) { Text(labels[$0] ?? ($0.isEmpty ? "—" : $0)).tag($0) }
            }
            .labelsHidden()
            .modifier(PickerWidth(width: width))
        }
    }

    private func table(_ e: CfgExplain) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                if !e.databases.isEmpty {
                    sectionHeader("DATABASES · per-branch")
                    ForEach(e.databases.sorted(by: { $0.key < $1.key }), id: \.key) { k, v in
                        row(key: k, value: v, badge: "db", badgeColor: Theme.tool)
                    }
                }
                sectionHeader(e.port > 0 ? "ENVIRONMENT · port \(e.port)" : "ENVIRONMENT")
                ForEach(e.env) { p in
                    row(key: p.key, value: p.value, badge: sourceText(p.source), badgeColor: sourceColor(p.source), secret: p.secret)
                }
                if e.env.isEmpty {
                    Text("no env for this service").font(.system(size: 12)).foregroundStyle(Theme.dim).padding(16)
                }
            }
            .padding(.bottom, 12)
        }
    }

    private func sectionHeader(_ t: String) -> some View {
        Text(t).font(.system(size: 10, weight: .semibold)).kerning(0.5).foregroundStyle(Theme.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 20).padding(.top, 14).padding(.bottom, 6)
    }

    private func row(key: String, value: String, badge: String, badgeColor: Color, secret: Bool = false) -> some View {
        let hidden = secret && !revealedEnv.contains(key) && !value.isEmpty
        let shown = hidden ? String(repeating: "•", count: 12) : (value.isEmpty ? "—" : value)
        return VStack(spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(key).font(Theme.mono(11.5, .medium)).foregroundStyle(Theme.fg)
                    .frame(width: 210, alignment: .leading).lineLimit(1).truncationMode(.middle)
                Text(shown).font(Theme.mono(11.5)).foregroundStyle(hidden ? Theme.dim : Theme.fgMuted)
                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).lineLimit(1).truncationMode(.middle)
                if secret && !value.isEmpty {
                    Button { toggleReveal(key) } label: {
                        Image(systemName: revealedEnv.contains(key) ? "eye.slash" : "eye").font(.system(size: 10))
                    }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted).help("Reveal value")
                }
                if !value.isEmpty { CopyMini(text: value) }
                Text(badge).font(.system(size: 9.5, weight: .medium)).foregroundStyle(badgeColor)
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .background(badgeColor.opacity(0.12), in: Capsule())
                    .frame(width: 88, alignment: .trailing).fixedSize()
            }
            .padding(.horizontal, 20).padding(.vertical, 5)
            Divider().overlay(Theme.borderSoft.opacity(0.35)).padding(.leading, 20)
        }
    }

    private func toggleReveal(_ key: String) {
        if revealedEnv.contains(key) { revealedEnv.remove(key) } else { revealedEnv.insert(key) }
    }

    private func sourceText(_ s: String) -> String {
        if s.isEmpty { return "resolved" }
        if s.hasPrefix("preset:") { return String(s.dropFirst(7)) }
        return s
    }
    private func sourceColor(_ s: String) -> Color {
        if s == "own" { return Theme.accent }
        if s.hasPrefix("preset:") { return Theme.tool }
        return Theme.dim
    }

    private var placeholder: some View {
        VStack(spacing: 8) {
            Image(systemName: "list.bullet.rectangle").font(.system(size: 26)).foregroundStyle(Theme.dim)
            Text("Pick a repo + service to inspect its resolved env").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
        }.frame(maxWidth: .infinity, maxHeight: .infinity).padding(.top, 40)
    }

    private func boot() async {
        if branch.isEmpty { branch = state.workspaces.first(where: { $0.isMain })?.branch ?? state.workspaces.first?.branch ?? "main" }
        let envs = SettingsStore.environments()
        struct E: Decodable { var environments: [String] = [] }
        if let d = PomJSON.decode(E.self, from: envs) { profiles = d.environments.sorted() }
        let resp = SettingsStore.configExplain(repo: "", branch: "", svc: "", env: "")
        if let d = PomJSON.decode(CfgExplainResp.self, from: resp) {
            repos = d.repos
            if repo.isEmpty { repo = repos.first?.repo ?? "" }
            svc = services.first ?? ""
        }
        await reload()
    }

    private func reload() async {
        guard !repo.isEmpty else { return }
        loading = true
        defer { loading = false }
        let resp = SettingsStore.configExplain(repo: repo, branch: branch, svc: svc, env: profile)
        if let d = PomJSON.decode(CfgExplainResp.self, from: resp) {
            if !d.repos.isEmpty { repos = d.repos }
            explain = d.explain
            errText = d.explain == nil ? (String(data: resp, encoding: .utf8) ?? "no data").prefix(300).description : ""
        } else {
            explain = nil
            errText = "decode failed: " + (String(data: resp, encoding: .utf8) ?? "").prefix(300).description
        }
    }
}


private struct CfgFile: Decodable, Hashable {
    var name = ""; var path = ""; var root = false
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
        root = try c.decodeIfPresent(Bool.self, forKey: .root) ?? false
    }
    enum K: String, CodingKey { case name, path, root }
}
private struct CfgFilesResp: Decodable {
    var files: [CfgFile] = []
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        files = try c.decodeIfPresent([CfgFile].self, forKey: .files) ?? []
    }
    enum K: String, CodingKey { case files }
}

struct AdvancedSettings: View {
    @EnvironmentObject var state: AppState
    @State private var files: [CfgFile] = []
    @State private var nodes: [ConfigNode] = []
    @State private var path = ""            // selected file
    @State private var text = ""
    @State private var original = ""
    @State private var status = ""
    @State private var busy = false
    @State private var showExport = false
    @State private var showImport = false
    @State private var showNew = false
    @State private var newName = ""
    @State private var newError = ""
    @StateObject private var doctor = DoctorViewModel()

    private var dirty: Bool { text != original }

    var body: some View {
        HStack(spacing: 0) {
            sidebar.frame(width: 220)
            Divider().overlay(Theme.borderSoft)
            VStack(spacing: 0) {
                toolbar
                if state.agentModel != nil { agentBanner; Divider().overlay(Theme.borderSoft) }
                Divider().overlay(Theme.borderSoft)
                YAMLEditor(text: $text)
                    .background(Theme.bg)
                Divider().overlay(Theme.borderSoft)
                ConfigDoctorBar(vm: doctor)
            }
        }
        .task { await load() }
        .sheet(isPresented: $showExport) { ExportBundleSheet() }
        .sheet(isPresented: $showImport, onDismiss: { Task { await load() } }) { ImportBundleSheet().environmentObject(state) }
        .sheet(isPresented: $showNew) { newFileSheet }
        .onChange(of: state.agentModel == nil) { if state.agentModel == nil { Task { await load() } } }
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            ScrollView(.vertical) {
                LazyVStack(alignment: .leading, spacing: 1) {
                    ForEach(nodes) { node in
                        ConfigNodeRow(node: node, depth: 0, selected: path) { p in
                            path = p
                            Task { await loadFile() }
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 6)
            }
            Divider().overlay(Theme.borderSoft)
            Button { newName = ""; newError = ""; showNew = true } label: {
                Label("New file", systemImage: "plus").font(.system(size: 11.5))
            }
            .buttonStyle(.plain).foregroundStyle(Theme.accent)
            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 12).padding(.vertical, 8)
        }
        .background(Theme.bgSoft)
    }

    private var newFileSheet: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("New config fragment").font(.system(size: 13, weight: .semibold))
            Text("Created under pom.d/ and merged into pom.yml.").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            TextField("services.yml", text: $newName).textFieldStyle(.roundedBorder).frame(width: 260)
            if !newError.isEmpty { Text(newError).font(.system(size: 11)).foregroundStyle(Theme.danger) }
            HStack {
                Spacer()
                Button("Cancel") { showNew = false }.keyboardShortcut(.cancelAction)
                Button("Create") { Task { await createFile() } }
                    .buttonStyle(.borderedProminent).tint(Theme.accent)
                    .disabled(newName.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20).frame(width: 320)
    }

    private var agentBanner: some View {
        HStack(spacing: 8) {
            if state.agentRunning { Spinner(size: 11) }
            else { Image(systemName: "checkmark.seal.fill").font(.system(size: 11)).foregroundStyle(Theme.ok) }
            Text(state.agentRunning ? "\(state.agentTitle) running…" : "\(state.agentTitle) finished")
                .font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.fg)
            Spacer()
            Button("View") { state.reopenAgent() }.buttonStyle(.plain).font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.accent)
            if !state.agentRunning {
                IconButton("xmark", size: 10) { state.endAgent() }
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 7).background(Theme.accentSoft)
    }

    private var toolbar: some View {
        HStack(spacing: 10) {
            Text(path.isEmpty ? "No file" : (path as NSString).lastPathComponent)
                .font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.fg).lineLimit(1)
            if !status.isEmpty { Text(status).font(.system(size: 11)).foregroundStyle(Theme.fgMuted).lineLimit(1) }
            Spacer()
            if busy { Spinner() }
            Button { showImport = true } label: { Label("Import", systemImage: "square.and.arrow.down") }
                .buttonStyle(.bordered).controlSize(.small).disabled(busy || state.agentRunning)
                .help(state.agentRunning ? "An agent is running — open it from the banner and wait for it to finish" : "Import a config")
            Button { showExport = true } label: { Label("Export", systemImage: "square.and.arrow.up") }
                .buttonStyle(.bordered).controlSize(.small).disabled(busy)
            Button("Save") { Task { await save() } }
                .buttonStyle(.borderedProminent).tint(Theme.accent).controlSize(.small)
                .disabled(!dirty || busy || state.agentRunning)
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
    }

    private func load() async {
        let d = await SettingsStore.configFiles()
        if let r = PomJSON.decode(CfgFilesResp.self, from: d) {
            files = r.files
            nodes = ConfigTree.build(r.files.map { .init(rel: $0.name, path: $0.path) })
            if path.isEmpty { path = r.files.first?.path ?? "" }
        }
        await loadFile()
        await doctor.load()
        await state.refreshConfigHealth()
    }

    private func loadFile() async {
        guard !path.isEmpty else { return }
        struct Doc: Decodable { var path = ""; var yaml = "" }
        let p = path
        let d = await SettingsStore.configFileGet(path: p)
        if let r = PomJSON.decode(Doc.self, from: d) { text = r.yaml; original = r.yaml; status = "" }
    }

    private func createFile() async {
        let name = newName.trimmingCharacters(in: .whitespaces)
        let d = await SettingsStore.configFileCreate(name: name)
        struct R: Decodable { var ok = false; var error = ""; var path = "" }
        let r = PomJSON.decode(R.self, from: d)
        guard let r, r.ok else { newError = r?.error ?? "create failed"; return }
        showNew = false
        await load()
        path = r.path
        await loadFile()
    }

    private func save() async {
        busy = true; defer { busy = false }
        let p = path, y = text
        let d = await SettingsStore.configFileSet(path: p, yaml: y)
        struct R: Decodable { var ok = false; var error = ""; var note = "" }
        let r = PomJSON.decode(R.self, from: d)
        if let r, !r.ok {
            status = "Error: \(r.error)"
            return
        }
        original = text
        status = (r?.note.isEmpty ?? true) ? "Saved." : r!.note
        await SettingsStore.configReload()
        await doctor.load()
        await state.refreshConfigHealth()
    }

    private func exportYAML() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = (path as NSString).lastPathComponent
        panel.begin { resp in
            guard resp == .OK, let url = panel.url else { return }
            try? text.data(using: .utf8)?.write(to: url)
            status = "Exported."
        }
    }

    private func importYAML() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.allowedContentTypes = [.yaml, .text, .data]
        panel.begin { resp in
            guard resp == .OK, let url = panel.url, let s = try? String(contentsOf: url, encoding: .utf8) else { return }
            text = s
            status = "Loaded \(url.lastPathComponent) — review and Save."
        }
    }
}

private struct PickerWidth: ViewModifier {
    let width: CGFloat?
    func body(content: Content) -> some View {
        if let w = width { content.frame(width: w) } else { content.fixedSize() }
    }
}

private extension String {
    var urlq: String { addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? self }
}

