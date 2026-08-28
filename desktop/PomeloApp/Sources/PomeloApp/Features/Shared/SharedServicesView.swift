import SwiftUI

private struct SharedSvc: Decodable, Identifiable {
    var name = ""; var type = ""; var image = ""; var port = 0; var running = false; var url = ""
    var id: String { name }
}
private struct SharedResp: Decodable { var services: [SharedSvc] = [] }

private struct SharedInspect: Decodable {
    struct Mount: Decodable, Hashable { var src = ""; var dst = "" }
    struct Port: Decodable, Hashable { var host = ""; var container = ""; var proto = "" }
    struct Label: Decodable, Hashable { var key = ""; var value = "" }
    var name = "", image = "", id = "", status = "", started_at = "", ip = "", url = ""
    var cpu = "", mem = "", net = "", disk = ""
    var port = 0
    var running = false
    var ports: [Port] = []
    var mounts: [Mount] = []
    var labels: [Label] = []
}
private struct SharedLogs: Decodable { var running = false; var lines: [String] = [] }

private enum DetailTab: String, CaseIterable { case info = "Info", stats = "Stats", logs = "Logs" }

struct SharedServicesView: View {
    @EnvironmentObject var theme: ThemeManager
    @EnvironmentObject var state: AppState
    var onClose: () -> Void = {}
    @State private var services: [SharedSvc] = []
    @State private var selected: String?
    @State private var busy = false
    @State private var poll = true
    @State private var totalCPU = ""
    @State private var totalMem = ""

    private var runningCount: Int { services.filter(\.running).count }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.borderSoft)
            HStack(spacing: 0) {
                list.frame(width: 260)
                Divider().overlay(Theme.borderSoft)
                Group {
                    if let svc = services.first(where: { $0.name == selected }) { DetailPane(svc: svc).id(svc.name) } else { pickPrompt }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(width: 900, height: 560)
        .background(Theme.bg)
        .task { await pollLoop() }
        .onDisappear { poll = false }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "cylinder.split.1x2").font(.system(size: 13)).foregroundStyle(Theme.accent)
            VStack(alignment: .leading, spacing: 1) {
                Text("Shared services").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                Text("\(runningCount)/\(services.count) running · local docker"
                     + (totalCPU.isEmpty ? "" : " · CPU \(totalCPU) · \(totalMem)"))
                    .font(.system(size: 11)).foregroundStyle(Theme.dim)
            }
            Spacer()
            if busy { Spinner(size: 12) }
            Button { Task { await stack("up") } } label: { Text("Start all").font(.system(size: 12)) }
                .buttonStyle(.plain).foregroundStyle(Theme.accent).disabled(busy)
            Button { Task { await stack("stop") } } label: { Text("Stop all").font(.system(size: 12)) }
                .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).disabled(busy)
            Button { onClose() } label: { Image(systemName: "xmark").font(.system(size: 12)) }
                .buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
        }
        .padding(.horizontal, 20).padding(.vertical, 14)
    }

    private var list: some View {
        ScrollView {
            LazyVStack(spacing: 2) {
                ForEach(services) { svc in
                    Button { selected = svc.name } label: { listRow(svc) }
                        .buttonStyle(.plain)
                }
            }
            .padding(8)
        }
        .background(Theme.bgSoft)
    }

    private func listRow(_ svc: SharedSvc) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon(svc.type)).font(.system(size: 14)).foregroundStyle(svc.running ? Theme.accent : Theme.dim).frame(width: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text(svc.name).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.fg)
                Text(svc.image).font(Theme.mono(10)).foregroundStyle(Theme.dim).lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 4)
            Circle().fill(svc.running ? Theme.ok : Theme.dim).frame(width: 7, height: 7)
            actions(svc)
        }
        .padding(.horizontal, 8).padding(.vertical, 7)
        .background(selected == svc.name ? Theme.sel : .clear, in: RoundedRectangle(cornerRadius: 6))
    }

    private func actions(_ svc: SharedSvc) -> some View {
        HStack(spacing: 2) {
            if svc.running {
                iconBtn("arrow.clockwise", "Restart") { Task { await act(svc.name, "restart") } }
                iconBtn("stop.fill", "Stop") { Task { await act(svc.name, "stop") } }
            } else {
                iconBtn("play.fill", "Start") { Task { await act(svc.name, "start") } }
            }
        }
    }

    private func iconBtn(_ sym: String, _ tip: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: sym).font(.system(size: 10)).foregroundStyle(Theme.fgMuted).frame(width: 18, height: 18) }
            .buttonStyle(.plain).help(tip).disabled(busy)
    }

    private var pickPrompt: some View {
        VStack(spacing: 8) {
            Image(systemName: "cylinder.split.1x2").font(.system(size: 26)).foregroundStyle(Theme.dim)
            Text(services.isEmpty ? "No shared services configured" : "Select a service to inspect")
                .font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func icon(_ type: String) -> String {
        switch type {
        case "postgres": return "cylinder"
        case "redis": return "bolt.horizontal"
        case "minio": return "externaldrive"
        case "opensearch": return "magnifyingglass"
        default: return "shippingbox"
        }
    }

    private func load() async {
        let d = await SharedServicesStore.status()
        if let r = PomJSON.decode(SharedResp.self, from: d) {
            services = r.services
            if selected == nil { selected = r.services.first(where: \.running)?.name ?? r.services.first?.name }
        }
    }

    private func act(_ name: String, _ action: String) async {
        busy = true; defer { busy = false }
        await SharedServicesStore.action(name: name, action: action)
        await load()
    }

    private func stack(_ action: String) async {
        busy = true; defer { busy = false }
        await SharedServicesStore.stack(action: action)
        await load()
    }

    private func loadTotal() async {
        let d = await SharedServicesStore.stats(name: "")
        struct T: Decodable { var cpu = ""; var mem = "" }
        if let r = PomJSON.decode(T.self, from: d) { totalCPU = r.cpu; totalMem = r.mem }
    }

    private func pollLoop() async {
        while poll {
            if state.appActive { await load(); await loadTotal() }
            try? await Task.sleep(nanoseconds: 3_000_000_000)
        }
    }
}


private struct DetailPane: View {
    @EnvironmentObject var state: AppState
    let svc: SharedSvc
    @State private var tab: DetailTab = .info
    @State private var info: SharedInspect
    @State private var logs: [String] = []
    @State private var cpu: [Double] = []
    @State private var mem: [Double] = []
    @State private var poll = true
    private var name: String { svc.name }

    init(svc: SharedSvc) {
        self.svc = svc
        _info = State(initialValue: SharedInspect(name: svc.name, image: svc.image, url: svc.url, port: svc.port, running: svc.running))
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 4) {
                SegmentedTabs(tabs: DetailTab.allCases, selection: $tab, label: \.rawValue, accent: false)
                    .onChange(of: tab) {
                        if tab == .stats { Task { await refreshStats() } }
                        else if tab == .logs { Task { await refresh() } }
                    }
                Spacer()
            }
            .padding(.horizontal, 16).padding(.vertical, 10)
            Divider().overlay(Theme.borderSoft)
            switch tab {
            case .info: infoTab
            case .stats: statsTab
            case .logs: logsTab
            }
        }
        .id(name)
        .task(id: name) { await refresh(); await pollLoop() }
    }

    private var infoTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                card {
                    kv("Name", info.name.isEmpty ? name : info.name)
                    if !info.id.isEmpty { kv("ID", String(info.id.prefix(12))) }
                    kv("Image", info.image.isEmpty ? svc.image : info.image)
                    kv("Status", statusText, color: svc.running ? Theme.ok : Theme.dim)
                }
                if !info.url.isEmpty || !info.ip.isEmpty {
                    card {
                        if !info.url.isEmpty { kv("Domain", info.url, color: Theme.accent) }
                        if !info.ip.isEmpty { kv("IP", info.ip) }
                    }
                }
                if !info.ports.isEmpty {
                    tableSection("Port Forwards", cols: ["Host Port", "Container Port", "Protocol"],
                                 rows: info.ports.map { [$0.host.isEmpty ? "—" : $0.host, $0.container, $0.proto.uppercased()] })
                }
                if !info.mounts.isEmpty {
                    tableSection("Mounts", cols: ["Source", "Destination"],
                                 rows: info.mounts.map { [$0.src, $0.dst] })
                }
                if !info.labels.isEmpty {
                    tableSection("Labels", cols: ["Key", "Value"],
                                 rows: info.labels.map { [$0.key, $0.value] })
                }
            }
            .padding(20)
        }
    }

    private var statusText: String {
        if !svc.running { return "stopped" }
        let up = uptime(info.started_at)
        return up.isEmpty ? "running" : "Up \(up)"
    }

    private func uptime(_ iso: String) -> String {
        let f = ISO8601DateFormatter(); f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        guard let d = f.date(from: iso) ?? ISO8601DateFormatter().date(from: iso) else { return "" }
        let s = Int(Date().timeIntervalSince(d))
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s/60)m" }
        if s < 86400 { return "\(s/3600)h" }
        return "\(s/86400)d"
    }

    private func card<C: View>(@ViewBuilder _ rows: @escaping () -> C) -> some View {
        Card(cornerRadius: 12) { VStack(spacing: 0) { rows() } }
    }

    private func tableSection(_ title: String, cols: [String], rows: [[String]]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.fg)
            VStack(spacing: 0) {
                HStack(spacing: 12) {
                    ForEach(Array(cols.enumerated()), id: \.offset) { _, c in
                        Text(c).font(.system(size: 10.5, weight: .medium)).foregroundStyle(Theme.muted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(.horizontal, 14).padding(.vertical, 8)
                Divider().overlay(Theme.borderSoft)
                ForEach(Array(rows.enumerated()), id: \.offset) { i, r in
                    HStack(spacing: 12) {
                        ForEach(Array(r.enumerated()), id: \.offset) { _, cell in
                            Text(cell).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                        }
                    }
                    .padding(.horizontal, 14).padding(.vertical, 7)
                    if i < rows.count - 1 { Divider().overlay(Theme.borderSoft.opacity(0.4)) }
                }
            }
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.borderSoft))
        }
    }

    private var statsTab: some View {
        VStack {
            if svc.running {
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
                    MetricCard(title: "CPU", value: info.cpu.isEmpty ? "—" : info.cpu, color: Theme.danger, samples: cpu, maxHint: 100)
                    MetricCard(title: "Memory", value: memValue, color: Theme.accent, samples: mem, maxHint: nil)
                    MetricCard(title: "Network", value: info.net.isEmpty ? "—" : info.net, color: Theme.ok, samples: [])
                    MetricCard(title: "Disk", value: info.disk.isEmpty ? "—" : info.disk, color: Theme.tool, samples: [])
                }
                .padding(16)
            } else {
                Text("Stopped — no stats").font(.system(size: 12)).foregroundStyle(Theme.dim)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            Spacer(minLength: 0)
        }
    }

    private var memValue: String { info.mem.split(separator: "/").first.map { $0.trimmingCharacters(in: .whitespaces) } ?? "—" }

    private var logsTab: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    ForEach(Array(logs.enumerated()), id: \.offset) { i, ln in
                        Text(ln).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted)
                            .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).id(i)
                    }
                    if logs.isEmpty {
                        Text("No logs.").font(.system(size: 12)).foregroundStyle(Theme.dim).padding(12)
                    }
                }
                .padding(12)
            }
            .background(Theme.bg)
            .onChange(of: logs.count) { if let l = logs.indices.last { proxy.scrollTo(l, anchor: .bottom) } }
        }
    }

    private func kv(_ k: String, _ v: String, color: Color = Theme.fg) -> some View {
        HStack(spacing: 12) {
            Text(k).font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
            Spacer(minLength: 12)
            Text(v).font(Theme.mono(11.5)).foregroundStyle(color).textSelection(.enabled)
                .lineLimit(1).truncationMode(.middle).frame(maxWidth: 360, alignment: .trailing)
        }
        .padding(.horizontal, 14).padding(.vertical, 9)
        .overlay(Rectangle().fill(Theme.borderSoft.opacity(0.4)).frame(height: 1).padding(.horizontal, 14), alignment: .bottom)
    }

    private func refresh() async {
        let n = name
        let d = await SharedServicesStore.inspect(name: n)
        if let r = PomJSON.decode(SharedInspect.self, from: d) { info = r }
        guard tab == .logs else { return }
        let l = await SharedServicesStore.logs(name: n, lines: 300)
        if let r = PomJSON.decode(SharedLogs.self, from: l) { logs = r.lines }
    }

    private func refreshStats() async {
        struct S: Decodable { var running = false; var cpu = ""; var mem = ""; var net = ""; var disk = "" }
        let n = name
        let d = await SharedServicesStore.stats(name: n)
        guard let r = PomJSON.decode(S.self, from: d), r.running else { return }
        info.cpu = r.cpu; info.mem = r.mem; info.net = r.net; info.disk = r.disk
        if let c = parsePct(r.cpu) { cpu = Array((cpu + [c]).suffix(40)) }
        if let m = parseMem(r.mem) { mem = Array((mem + [m]).suffix(40)) }
    }

    private func parsePct(_ s: String) -> Double? { Double(s.replacingOccurrences(of: "%", with: "").trimmingCharacters(in: .whitespaces)) }
    private func parseMem(_ s: String) -> Double? {
        guard let used = s.split(separator: "/").first?.trimmingCharacters(in: .whitespaces) else { return nil }
        let units: [(String, Double)] = [("GiB", 1073741824), ("MiB", 1048576), ("KiB", 1024), ("GB", 1e9), ("MB", 1e6), ("kB", 1e3), ("B", 1)]
        for (u, mul) in units where used.hasSuffix(u) {
            if let n = Double(used.dropLast(u.count)) { return n * mul }
        }
        return Double(used)
    }

    private func pollLoop() async {
        while poll {
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            if Task.isCancelled { break }
            guard state.appActive else { continue }
            if tab == .stats { await refreshStats() } else { await refresh() }
        }
    }
}
