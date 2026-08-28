import SwiftUI

struct ProcInfo: Decodable, Identifiable, Equatable {
    var pid = 0
    var label = ""
    var name = ""
    var ws_key = ""
    var branch = ""
    var repo = ""
    var svc = ""
    var kind = ""
    var cpu = 0.0
    var ram_mb = 0.0
    var id: Int { pid }
}
struct PsTotal: Decodable, Equatable { var cpu = 0.0; var ram_mb = 0.0; var procs = 0 }
struct PsResponse: Decodable { var processes: [ProcInfo] = []; var total = PsTotal() }

struct ActivityView: View {
    @EnvironmentObject var state: AppState
    @EnvironmentObject var theme: ThemeManager
    var scopeWsKey: String? = nil
    var onClose: () -> Void = {}

    @State private var procs: [ProcInfo] = []
    @State private var loaded = false
    @State private var total = PsTotal()
    @State private var poll: Task<Void, Never>?
    @State private var cpuHist: [Double] = []
    @State private var ramHist: [Double] = []
    @State private var sortMode: SortMode = .cpu
    @State private var collapsed: Set<String> = []

    enum SortMode: String, CaseIterable { case cpu = "CPU", mem = "Mem", name = "Name" }

    private func sortRows(_ r: [ProcInfo]) -> [ProcInfo] {
        switch sortMode {
        case .cpu: return r.sorted { $0.cpu > $1.cpu }
        case .mem: return r.sorted { $0.ram_mb > $1.ram_mb }
        case .name: return r.sorted { procName($0).localizedCaseInsensitiveCompare(procName($1)) == .orderedAscending }
        }
    }
    private func procName(_ p: ProcInfo) -> String {
        if !p.svc.isEmpty { return p.svc }
        if !p.name.isEmpty { return p.name }
        return p.label.isEmpty ? "process" : p.label
    }
    private func sortGroups(_ g: [(String, [ProcInfo])]) -> [(String, [ProcInfo])] {
        switch sortMode {
        case .name: return g.sorted { $0.0.localizedCaseInsensitiveCompare($1.0) == .orderedAscending }
        case .mem: return g.sorted { agg($0.1, \.ram_mb) > agg($1.1, \.ram_mb) }
        case .cpu: return g.sorted { agg($0.1, \.cpu) > agg($1.1, \.cpu) }
        }
    }
    private func agg(_ rows: [ProcInfo], _ kp: KeyPath<ProcInfo, Double>) -> Double { rows.reduce(0) { $0 + $1[keyPath: kp] } }

    private var shown: [ProcInfo] {
        sortRows(procs.filter { scopeWsKey == nil || $0.ws_key == scopeWsKey })
    }
    private var groups: [(String, [ProcInfo])] {
        let g = Dictionary(grouping: shown) { $0.branch.isEmpty ? "other" : $0.branch }
        return sortGroups(g.map { ($0.key, sortRows($0.value)) })
    }
    private func repoGroups(_ rows: [ProcInfo]) -> [(String, [ProcInfo])] {
        let g = Dictionary(grouping: rows) { $0.repo.isEmpty ? "workspace" : $0.repo }
        return sortGroups(g.map { ($0.key, sortRows($0.value)) })
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                SectionLabel(text: scopeWsKey == nil ? "ACTIVITY · ALL" : "ACTIVITY", size: 11)
                Spacer()
                Text(String(format: "CPU %.0f%%", scopedTotal.cpu)).font(Theme.mono(11)).foregroundStyle(cpuColor(scopedTotal.cpu))
                Text(String(format: "· %.0f MB", scopedTotal.ram)).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted)
                Text("· \(shown.count)").font(Theme.mono(11)).foregroundStyle(Theme.dim)
                Picker("", selection: $sortMode) {
                    ForEach(SortMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented).labelsHidden().frame(width: 150).controlSize(.small).padding(.leading, 8)
                IconButton("xmark", size: 11, action: onClose).padding(.leading, 6)
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
            Divider().overlay(Theme.borderSoft)

            if shown.isEmpty && !loaded {
                LoadingView(text: "loading processes…")
            } else if shown.isEmpty {
                EmptyStateView(icon: "bolt.slash", title: "Nothing running")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        if scopeWsKey == nil {
                            ForEach(groups, id: \.0) { branch, rows in
                                let bkey = "b:" + branch
                                treeHeader(branch, key: bkey, rows: rows, indent: 0, accent: true)
                                if !collapsed.contains(bkey) {
                                    // id must include the branch — every workspace has a "workspace" group; a shared repo id blanks the duplicates in the LazyVStack
                                    ForEach(repoGroups(rows).map { (rkey: "r:" + branch + "/" + $0.0, repo: $0.0, svcs: $0.1) }, id: \.rkey) { g in
                                        treeHeader(g.repo, key: g.rkey, rows: g.svcs, indent: 1, accent: false)
                                        if !collapsed.contains(g.rkey) { ForEach(g.svcs) { procRow($0, indent: 2) } }
                                    }
                                }
                            }
                        } else {
                            ForEach(repoGroups(shown), id: \.0) { repo, svcs in
                                let rkey = "r:" + repo
                                treeHeader(repo, key: rkey, rows: svcs, indent: 0, accent: false)
                                if !collapsed.contains(rkey) { ForEach(svcs) { procRow($0, indent: 1) } }
                            }
                        }
                    }.padding(.bottom, 10)
                }
            }

            Divider().overlay(Theme.borderSoft)
            HStack(spacing: 10) {
                MetricCard(title: "CPU", value: String(format: "%.0f%%", scopedTotal.cpu),
                           color: Theme.danger, samples: cpuHist, maxHint: 100)
                MetricCard(title: "Memory", value: memLabel(scopedTotal.ram),
                           color: Color(hex: 0x4aa3ff), samples: ramHist, maxHint: nil)
            }
            .padding(12)
        }
        .frame(minWidth: 460, minHeight: 420)
        .background(Theme.bgSoft)
        .onExitCommand { onClose() }
        .task { await load(); startPoll() }
        .onDisappear { poll?.cancel() }
    }

    private func memLabel(_ mb: Double) -> String {
        mb >= 1024 ? String(format: "%.2f GB", mb / 1024) : String(format: "%.0f MB", mb)
    }

    private var scopedTotal: (cpu: Double, ram: Double) {
        if scopeWsKey == nil { return (total.cpu, total.ram_mb) }
        return (shown.reduce(0) { $0 + $1.cpu }, shown.reduce(0) { $0 + $1.ram_mb })
    }

    private func treeHeader(_ label: String, key: String, rows: [ProcInfo], indent: Int, accent: Bool) -> some View {
        let cpu = rows.reduce(0) { $0 + $1.cpu }, ram = rows.reduce(0) { $0 + $1.ram_mb }
        let isCollapsed = collapsed.contains(key)
        return Button {
            if isCollapsed { collapsed.remove(key) } else { collapsed.insert(key) }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: isCollapsed ? "chevron.right" : "chevron.down")
                    .font(.system(size: 8)).foregroundStyle(Theme.dim).frame(width: 10)
                Image(systemName: accent ? "square.stack.3d.up.fill" : "folder.fill")
                    .font(.system(size: accent ? 10 : 9)).foregroundStyle(accent ? Theme.accent : Theme.fgMuted).frame(width: 14)
                Text(label).font(Theme.mono(accent ? 11 : 11.5, .semibold)).foregroundStyle(accent ? Theme.accent : Theme.fg).lineLimit(1)
                Spacer(minLength: 8)
                Text(String(format: "%.0f%%", cpu)).font(Theme.mono(10.5)).foregroundStyle(Theme.dim).frame(width: 46, alignment: .trailing)
                Text(memShort(ram)).font(Theme.mono(10.5)).foregroundStyle(Theme.dim).frame(width: 66, alignment: .trailing)
            }
            .contentShape(Rectangle())
            .padding(.leading, CGFloat(16 + indent * 16)).padding(.trailing, 16)
            .padding(.top, indent == 0 ? 10 : 4).padding(.bottom, 3)
        }.buttonStyle(.plain)
        .contextMenu {
            Button(role: .destructive) { for p in rows { stopProc(p) } } label: {
                Label(accent ? "Stop all in \(label)" : "Stop all \(label)", systemImage: "stop.fill")
            }
        }
    }

    private func procRow(_ p: ProcInfo, indent: Int) -> some View {
        HStack(spacing: 8) {
            Text("└").font(Theme.mono(10)).foregroundStyle(Theme.dim.opacity(0.6)).frame(width: 14)
            Circle().fill(p.cpu > 20 ? Theme.warn : Theme.dim).frame(width: 6, height: 6)
            Text(procName(p)).font(.system(size: 12)).foregroundStyle(Theme.fg).lineLimit(1)
            Spacer(minLength: 8)
            Text(String(format: "%.0f%%", p.cpu)).font(Theme.mono(11)).foregroundStyle(cpuColor(p.cpu)).frame(width: 46, alignment: .trailing)
            Text(memShort(p.ram_mb)).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted).frame(width: 66, alignment: .trailing)
        }
        .padding(.leading, CGFloat(16 + indent * 16)).padding(.trailing, 16).padding(.vertical, 4)
        .contentShape(Rectangle())
        .contextMenu {
            Button(role: .destructive) { stopProc(p) } label: { Label("Stop \(procName(p))", systemImage: "stop.fill") }
        }
        .help("Right-click to stop \(procName(p))")
    }

    private func stopProc(_ p: ProcInfo) {
        if !p.repo.isEmpty, !p.svc.isEmpty {
            let isMain = p.ws_key.hasPrefix("main:")
            let ref: [String: Any] = ["branch": p.branch, "is_main": isMain, "repo": p.repo, "svc": p.svc]
            let body = (try? JSONSerialization.data(withJSONObject: ref)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
            Task {
                await ActivityStore.serviceStop(refJSON: body)
                await load()
            }
        } else if !p.label.isEmpty {
            let holder = p.label
            Task {
                await ActivityStore.paneKill(paneID: "pty:" + holder)
                await load()
            }
        }
    }

    private func memShort(_ mb: Double) -> String {
        mb >= 1024 ? String(format: "%.1f GB", mb / 1024) : String(format: "%.0f MB", mb)
    }

    private func cpuColor(_ c: Double) -> Color { c > 80 ? Theme.danger : c > 30 ? Theme.warn : Theme.fgMuted }

    private func load() async {
        let d = await ActivityStore.ps()
        let fresh = PomJSON.decode(PsResponse.self, from: d)
        loaded = true
        if let fresh {
            procs = fresh.processes; total = fresh.total
            let t = scopedTotal
            cpuHist.append(t.cpu); if cpuHist.count > 60 { cpuHist.removeFirst(cpuHist.count - 60) }
            ramHist.append(t.ram); if ramHist.count > 60 { ramHist.removeFirst(ramHist.count - 60) }
        }
    }

    private func startPoll() {
        poll?.cancel()
        poll = Task { [weak state] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                if state?.appActive == true { await load() }
            }
        }
    }
}

struct MetricCard: View {
    let title: String
    let value: String
    let color: Color
    let samples: [Double]
    var maxHint: Double?

    var body: some View {
        Card(cornerRadius: 10) {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text(title).font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.fgMuted)
                    Spacer()
                    Text(value).font(Theme.mono(12.5, .semibold)).foregroundStyle(color)
                }
                Sparkline(samples: samples, color: color, maxHint: maxHint).frame(height: 36)
            }
            .padding(10)
            .frame(maxWidth: .infinity)
        }
    }
}

struct Sparkline: View {
    let samples: [Double]
    let color: Color
    var maxHint: Double?

    var body: some View {
        GeometryReader { geo in
            let w = geo.size.width, h = geo.size.height
            let peak = max(samples.max() ?? 0, maxHint ?? 0, 0.001)
            let n = samples.count
            let stepX = n > 1 ? w / CGFloat(n - 1) : w
            let pts = samples.enumerated().map { i, v in
                CGPoint(x: CGFloat(i) * stepX, y: h - CGFloat(min(v / peak, 1)) * (h - 2) - 1)
            }
            ZStack {
                if pts.count > 1 {
                    Path { p in
                        p.move(to: CGPoint(x: pts[0].x, y: h))
                        for pt in pts { p.addLine(to: pt) }
                        p.addLine(to: CGPoint(x: pts.last!.x, y: h))
                        p.closeSubpath()
                    }
                    .fill(LinearGradient(colors: [color.opacity(0.32), color.opacity(0.02)],
                                         startPoint: .top, endPoint: .bottom))
                    Path { p in
                        p.move(to: pts[0])
                        for pt in pts.dropFirst() { p.addLine(to: pt) }
                    }
                    .stroke(color, style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
                } else {
                    Rectangle().fill(color.opacity(0.06))
                }
            }
        }
    }
}
