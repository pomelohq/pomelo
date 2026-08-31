import SwiftUI

var appVersion: String {
    Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0"
}

private struct DeviceStat { var ws = 0, running = 0, agents = 0 }

private struct ClaudeUsage: Decodable {
    struct Win: Decodable {
        var pct: Double = 0
        var resetsAt: Int64 = 0
        init() {}
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            pct = try c.decodeIfPresent(Double.self, forKey: .pct) ?? 0
            resetsAt = try c.decodeIfPresent(Int64.self, forKey: .resetsAt) ?? 0
        }
        enum K: String, CodingKey { case pct, resetsAt = "resets_at" }
    }
    struct Account: Decodable {
        var email = "", plan = "", org = ""
        init() {}
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            email = try c.decodeIfPresent(String.self, forKey: .email) ?? ""
            plan = try c.decodeIfPresent(String.self, forKey: .plan) ?? ""
            org = try c.decodeIfPresent(String.self, forKey: .org) ?? ""
        }
        enum K: String, CodingKey { case email, plan, org }
    }
    var ok = false
    var session = Win()
    var weekly = Win()
    var account = Account()
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        ok = try c.decodeIfPresent(Bool.self, forKey: .ok) ?? false
        session = try c.decodeIfPresent(Win.self, forKey: .session) ?? Win()
        weekly = try c.decodeIfPresent(Win.self, forKey: .weekly) ?? Win()
        account = try c.decodeIfPresent(Account.self, forKey: .account) ?? Account()
    }
    enum K: String, CodingKey { case ok, session, weekly, account }
}

public struct RootView: View {
    @EnvironmentObject var store: DeviceStore
    @EnvironmentObject var theme: ThemeManager
    @State private var pairing = false
    @State private var reachable: [String: Bool] = [:]
    @State private var stats: [String: DeviceStat] = [:]
    @State private var usage: [String: ClaudeUsage] = [:]
    @State private var renaming: PairedDevice?
    @State private var renameText = ""
    @State private var editingHost: PairedDevice?
    @State private var hostText = ""
    @State private var portText = ""
    @State private var showSettings = false

    public init() {}

    public var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    topBar
                    Text("Welcome back").font(Theme.ui(28, .bold)).foregroundStyle(Theme.fg)
                    if !store.devices.isEmpty { statCards }
                    desktopsSection
                    accountUsageSection
                    quickActions
                }
                .padding(.horizontal, 16).padding(.top, 8).padding(.bottom, 24)
            }
            .background(Theme.bg.ignoresSafeArea())
            .refreshable { await refreshOnce() }
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(for: PairedDevice.self) { d in
                DashboardView(client: RemoteClient(device: d))
            }
            .sheet(isPresented: $pairing) {
                PairingView { device in store.add(device); pairing = false }
            }
            .sheet(isPresented: $showSettings) { SettingsView() }
            .alert("Rename Mac", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
                TextField("Name", text: $renameText)
                Button("Cancel", role: .cancel) { renaming = nil }
                Button("Save") { if let d = renaming { store.rename(d, to: renameText) }; renaming = nil }
            }
            .alert("Change address", isPresented: Binding(get: { editingHost != nil }, set: { if !$0 { editingHost = nil } })) {
                TextField("100.x.y.z or 192.168.x.x", text: $hostText)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("Port (e.g. 8768)", text: $portText).keyboardType(.numberPad)
                Button("Cancel", role: .cancel) { editingHost = nil }
                Button("Save") { if let d = editingHost { store.setAddress(d, host: hostText, port: portText) }; editingHost = nil }
            } message: {
                Text("Switch this Mac's IP/port (Tailscale 100.x away, LAN 192.168.x at home; dev app uses 8769, prod 8768). The pinned certificate stays valid.")
            }
        }
        .tint(Theme.accent)
        .preferredColorScheme(activeThemeMode == .light ? .light : .dark)
        .task { await pingLoop() }
    }

    private var topBar: some View {
        HStack(spacing: 8) {
            Image("PomeloMark").resizable().scaledToFit().frame(width: 24, height: 24)
                .clipShape(RoundedRectangle(cornerRadius: 6))
            Text("Pomelo").font(Theme.ui(20, .bold)).foregroundStyle(Theme.fg)
            Text("v\(appVersion)").font(Theme.mono(9)).foregroundStyle(Theme.fgMuted).padding(.top, 4)
            Spacer()
            Button { pairing = true } label: {
                Image(systemName: "qrcode.viewfinder").font(.system(size: 18)).foregroundStyle(Theme.fgMuted)
            }
            Button { showSettings = true } label: {
                Image(systemName: "gearshape").font(.system(size: 18)).foregroundStyle(Theme.fgMuted)
            }
        }
    }

    private var statCards: some View {
        let total = stats.values.reduce(into: DeviceStat()) { a, s in a.ws += s.ws; a.running += s.running; a.agents += s.agents }
        return HStack(spacing: 10) {
            statCard("square.grid.2x2", "\(total.ws)", "Worktrees")
            statCard("bolt.fill", "\(total.running)", "Running")
            statCard("cpu", "\(total.agents)", "Agents")
        }
    }

    private func statCard(_ icon: String, _ value: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Image(systemName: icon).font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                .frame(width: 26, height: 26).background(Theme.chip, in: RoundedRectangle(cornerRadius: 7))
            Text(value).font(Theme.ui(20, .bold)).foregroundStyle(Theme.fg)
            Text(label).font(Theme.ui(10)).foregroundStyle(Theme.fgMuted)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.borderSoft, lineWidth: 1))
    }

    @ViewBuilder private var desktopsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "Desktops")
            if store.devices.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "desktopcomputer").font(.system(size: 30)).foregroundStyle(Theme.fgMuted)
                    Text("No Macs paired").font(Theme.ui(14, .semibold)).foregroundStyle(Theme.fg)
                    Text("Scan the QR in Pomelo > Settings > Network > Remote control.")
                        .font(Theme.ui(12)).foregroundStyle(Theme.fgMuted)
                        .multilineTextAlignment(.center).padding(.horizontal, 24)
                }
                .frame(maxWidth: .infinity).padding(.vertical, 24)
                .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 14))
                .overlay(RoundedRectangle(cornerRadius: 14).stroke(Theme.borderSoft, lineWidth: 1))
            } else {
                ForEach(store.devices) { d in
                    NavigationLink(value: d) { desktopCard(d) }.buttonStyle(.plain)
                }
            }
        }
    }

    private func desktopCard(_ d: PairedDevice) -> some View {
        let up = reachable[d.id]
        let s = stats[d.id]
        return HStack(spacing: 14) {
            RoundedRectangle(cornerRadius: 12).fill(Theme.chip)
                .frame(width: 48, height: 48)
                .overlay(Image(systemName: "desktopcomputer").font(.system(size: 20)).foregroundStyle(Theme.accent))
            VStack(alignment: .leading, spacing: 4) {
                Text(d.name.isEmpty ? d.host : d.name).font(Theme.ui(16, .bold)).foregroundStyle(Theme.fg)
                HStack(spacing: 6) {
                    Circle().fill(up == true ? Theme.ok : (up == false ? Theme.danger : Theme.dim)).frame(width: 7, height: 7)
                    Text(statusLine(up, s)).font(Theme.ui(12)).foregroundStyle(Theme.fgMuted)
                        .lineLimit(1).truncationMode(.tail)
                }
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right").font(.system(size: 13)).foregroundStyle(Theme.dim)
        }
        .padding(14)
        .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Theme.borderSoft, lineWidth: 1))
        .contextMenu {
            Button { renaming = d; renameText = d.name } label: { Label("Rename", systemImage: "pencil") }
            Button { editingHost = d; hostText = d.host; portText = d.port } label: { Label("Change address", systemImage: "network") }
            Button(role: .destructive) { store.remove(d) } label: { Label("Remove", systemImage: "trash") }
        }
    }

    private func statusLine(_ up: Bool?, _ s: DeviceStat?) -> String {
        guard up == true else { return up == false ? "Offline" : "Connecting..." }
        if let s = s { return "Connected · \(s.ws) worktrees · \(s.running) active" }
        return "Connected"
    }

    @ViewBuilder private var accountUsageSection: some View {
        if let u = usage.values.first(where: { $0.ok }) {
            VStack(alignment: .leading, spacing: 10) {
                SectionLabel(text: "Account usage")
                VStack(alignment: .leading, spacing: 12) {
                    HStack(spacing: 10) {
                        Image(systemName: "sparkle").font(.system(size: 15)).foregroundStyle(Theme.accent)
                            .frame(width: 34, height: 34).background(Theme.chip, in: RoundedRectangle(cornerRadius: 9))
                        Text(u.account.email.isEmpty ? "Claude" : u.account.email)
                            .font(Theme.ui(14, .semibold)).foregroundStyle(Theme.fg).lineLimit(1)
                        Spacer()
                        let tag = u.account.plan.isEmpty ? u.account.org : u.account.plan
                        if !tag.isEmpty {
                            Text(tag).font(Theme.ui(10, .medium)).foregroundStyle(Theme.fgMuted)
                                .padding(.horizontal, 7).padding(.vertical, 2).background(Theme.chip, in: Capsule())
                        }
                    }
                    usageBar("Session", u.session)
                    usageBar("Weekly", u.weekly)
                }
                .padding(14)
                .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 14))
                .overlay(RoundedRectangle(cornerRadius: 14).stroke(Theme.borderSoft, lineWidth: 1))
            }
        }
    }

    private func usageBar(_ label: String, _ win: ClaudeUsage.Win) -> some View {
        let frac = min(1, max(0, win.pct > 1 ? win.pct / 100 : win.pct))
        let color: Color = frac >= 0.9 ? Theme.danger : (frac >= 0.7 ? Theme.warn : Theme.ok)
        return VStack(alignment: .leading, spacing: 5) {
            Text(label).font(Theme.ui(13, .semibold)).foregroundStyle(Theme.fg)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.chip).frame(height: 6)
                    Capsule().fill(color).frame(width: geo.size.width * frac, height: 6)
                }
            }.frame(height: 6)
            HStack {
                Text("\(Int(frac * 100))% used").font(Theme.ui(11)).foregroundStyle(Theme.fgMuted)
                Spacer()
                if let r = resetsIn(win.resetsAt) {
                    Text(r).font(Theme.ui(11)).foregroundStyle(Theme.dim)
                }
            }
        }
    }

    private func resetsIn(_ ts: Int64) -> String? {
        guard ts > 0 else { return nil }
        let delta = ts - Int64(Date().timeIntervalSince1970)
        guard delta > 0 else { return nil }
        let d = delta / 86400, h = (delta % 86400) / 3600, m = (delta % 3600) / 60
        if d > 0 { return "Resets in \(d)d \(h)h" }
        if h > 0 { return "Resets in \(h)h \(m)m" }
        return "Resets in \(m)m"
    }

    private var quickActions: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "Quick actions")
            Button { pairing = true } label: {
                HStack(spacing: 10) {
                    Image(systemName: "qrcode").font(.system(size: 16)).foregroundStyle(Theme.accent)
                        .frame(width: 34, height: 34).background(Theme.chip, in: RoundedRectangle(cornerRadius: 9))
                    Text("Pair Desktop").font(Theme.ui(14, .semibold)).foregroundStyle(Theme.fg)
                    Spacer()
                }
                .padding(12)
                .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.borderSoft, lineWidth: 1))
            }.buttonStyle(.plain)
        }
    }

    private func pingLoop() async {
        while !Task.isCancelled {
            await refreshOnce()
            try? await Task.sleep(nanoseconds: 5_000_000_000)
        }
    }

    private func refreshOnce() async {
        for d in store.devices {
            let client = RemoteClient(device: d)
            let ok = (try? await client.ping()) ?? false
            reachable[d.id] = ok
            if ok, let data = try? await client.query("workspaces", ["git": false]),
               let p = PomJSON.decode(WorkspacesPayload.self, from: data) {
                let ws = p.workspaces
                stats[d.id] = DeviceStat(ws: ws.count,
                                         running: ws.filter { $0.running > 0 }.count,
                                         agents: ws.filter { $0.agents.contains { agentOrbActive($0.state) } }.count)
            }
            if ok, let u = try? await client.claudeUsage(),
               let cu = PomJSON.decode(ClaudeUsage.self, from: u) {
                usage[d.id] = cu
            }
        }
    }
}

struct SettingsView: View {
    @EnvironmentObject var store: DeviceStore
    @EnvironmentObject var theme: ThemeManager
    @Environment(\.dismiss) private var dismiss
    @State private var renaming: PairedDevice?
    @State private var renameText = ""

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    appearanceSection
                    pairedSection
                    aboutSection
                }
                .padding(16)
            }
            .background(Theme.bg.ignoresSafeArea())
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSoft, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .alert("Rename Mac", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
                TextField("Name", text: $renameText)
                Button("Cancel", role: .cancel) { renaming = nil }
                Button("Save") { if let d = renaming { store.rename(d, to: renameText) }; renaming = nil }
            }
        }
        .preferredColorScheme(activeThemeMode == .light ? .light : .dark)
    }

    @ViewBuilder private var appearanceSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "Appearance")
            VStack(spacing: 0) {
                ForEach(ThemeMode.allCases, id: \.self) { m in
                    Button { theme.mode = m } label: {
                        HStack(spacing: 12) {
                            Image(systemName: themeIcon(m)).font(.system(size: 16))
                                .foregroundStyle(theme.mode == m ? Theme.accent : Theme.fgMuted)
                                .frame(width: 22)
                            Text(themeLabel(m)).font(Theme.ui(14)).foregroundStyle(Theme.fg)
                            Spacer()
                            if theme.mode == m {
                                Image(systemName: "checkmark").font(.system(size: 13, weight: .semibold))
                                    .foregroundStyle(Theme.accent)
                            }
                        }
                        .padding(.horizontal, 12).padding(.vertical, 12).contentShape(Rectangle())
                    }.buttonStyle(.plain)
                    if m != ThemeMode.allCases.last { Divider().overlay(Theme.borderSoft) }
                }
            }
            .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.borderSoft, lineWidth: 1))
        }
    }

    private func themeLabel(_ m: ThemeMode) -> String {
        switch m {
        case .dark: return "Dark"
        case .light: return "Light"
        case .sepia: return "Sepia"
        }
    }

    private func themeIcon(_ m: ThemeMode) -> String {
        switch m {
        case .dark: return "moon.fill"
        case .light: return "sun.max.fill"
        case .sepia: return "book.fill"
        }
    }

    @ViewBuilder private var pairedSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "Paired Macs")
            if store.devices.isEmpty {
                Text("No Macs paired. Scan the QR in Pomelo > Settings > Network > Remote control.")
                    .font(Theme.ui(12)).foregroundStyle(Theme.fgMuted)
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 12))
                    .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.borderSoft, lineWidth: 1))
            } else {
                VStack(spacing: 0) {
                    ForEach(store.devices) { d in
                        HStack(spacing: 12) {
                            Image(systemName: "desktopcomputer").font(.system(size: 18)).foregroundStyle(Theme.accent)
                                .frame(width: 34, height: 34).background(Theme.chip, in: RoundedRectangle(cornerRadius: 8))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(d.name.isEmpty ? d.host : d.name).font(Theme.ui(14, .semibold)).foregroundStyle(Theme.fg)
                                Text("\(d.host):\(d.port)").font(Theme.mono(11)).foregroundStyle(Theme.fgMuted)
                            }
                            Spacer()
                            Button { renaming = d; renameText = d.name } label: {
                                Image(systemName: "pencil").font(.system(size: 14)).foregroundStyle(Theme.fgMuted)
                            }
                            Button { store.remove(d) } label: {
                                Image(systemName: "trash").font(.system(size: 14)).foregroundStyle(Theme.danger)
                            }.padding(.leading, 6)
                        }
                        .padding(.horizontal, 12).padding(.vertical, 10)
                        if d.id != store.devices.last?.id { Divider().overlay(Theme.borderSoft) }
                    }
                }
                .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.borderSoft, lineWidth: 1))
            }
        }
    }

    @ViewBuilder private var aboutSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "About")
            VStack(spacing: 0) {
                aboutRow("App", "Pomelo Remote")
                Divider().overlay(Theme.borderSoft)
                aboutRow("Version", "v\(appVersion)")
                Divider().overlay(Theme.borderSoft)
                aboutRow("Reach", "LAN only (same network / VPN)")
            }
            .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.borderSoft, lineWidth: 1))
        }
    }

    private func aboutRow(_ k: String, _ v: String) -> some View {
        HStack {
            Text(k).font(Theme.ui(13)).foregroundStyle(Theme.fgSoft)
            Spacer()
            Text(v).font(Theme.ui(13)).foregroundStyle(Theme.fgMuted)
        }
        .padding(.horizontal, 12).padding(.vertical, 11)
    }
}
