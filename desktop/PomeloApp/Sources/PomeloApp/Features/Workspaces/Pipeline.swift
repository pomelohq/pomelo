import SwiftUI

struct PrepareEvent: Decodable {
    var stage = ""; var status = ""; var detail = ""; var ms: Int = 0
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        stage = try c.decodeIfPresent(String.self, forKey: .stage) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        detail = try c.decodeIfPresent(String.self, forKey: .detail) ?? ""
        ms = try c.decodeIfPresent(Int.self, forKey: .ms) ?? 0
    }
    enum K: String, CodingKey { case stage, status, detail, ms }
}

enum StageStatus: String { case pending, running, ok, failed, skipped }

struct PipeStage: Identifiable {
    let id: String
    let title: String
    var status: StageStatus = .pending
    var ms: Int = 0
    var detail = ""
}

struct PrepareMainPipelineView: View {
    @EnvironmentObject var theme: ThemeManager
    @Environment(\.dismiss) private var dismiss

    @State private var stages: [PipeStage] = [
        .init(id: "reset", title: "Reset databases"),
        .init(id: "migrate", title: "Migrate"),
        .init(id: "seed", title: "Seed"),
    ]
    @State private var skipSeed = false
    @State private var running = false
    @State private var finished = false
    @State private var streamID: Int32 = 0
    @State private var logs: [String] = []
    @State private var showLog = false

    private var overall: StageStatus {
        if stages.contains(where: { $0.status == .failed }) { return .failed }
        if finished { return .ok }
        return running ? .running : .pending
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                Image(systemName: "arrow.triangle.2.circlepath").font(.system(size: 13)).foregroundStyle(Theme.accent)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Prepare main").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text("Rebuild the golden-source databases (reset → migrate → seed)")
                        .font(.system(size: 11)).foregroundStyle(Theme.dim)
                }
                Spacer()
                Button { dismiss() } label: { Image(systemName: "xmark").font(.system(size: 12)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
            }
            .padding(.horizontal, 20).padding(.vertical, 14)
            Divider().overlay(Theme.borderSoft)

            HStack(spacing: 0) {
                ForEach(Array(stages.enumerated()), id: \.element.id) { i, st in
                    node(st)
                    if i < stages.count - 1 {
                        Rectangle().fill(Theme.borderSoft).frame(width: 26, height: 2)
                    }
                }
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 24)

            if showLog || !logs.isEmpty { logPanel }

            Spacer(minLength: 0)
            Divider().overlay(Theme.borderSoft)
            HStack(spacing: 10) {
                Toggle(isOn: $skipSeed) { Text("Skip seed").font(.system(size: 12)) }
                    .toggleStyle(.checkbox).disabled(running)
                Text("resets & rebuilds main's DBs").font(.system(size: 11)).foregroundStyle(Theme.dim)
                Spacer()
                if finished {
                    Label(overall == .failed ? "Finished with errors" : "Main prepared",
                          systemImage: overall == .failed ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                        .font(.system(size: 12)).foregroundStyle(overall == .failed ? Theme.danger : Theme.ok)
                    Button("Close") { dismiss() }.buttonStyle(.borderedProminent).tint(Theme.accent)
                } else {
                    Button { run() } label: { Text(running ? "Running…" : "Run prepare-main") }
                        .buttonStyle(.borderedProminent).tint(Theme.accent).disabled(running)
                        .rainbowShimmer(active: running, cornerRadius: 6)
                }
            }
            .padding(.horizontal, 20).padding(.vertical, 12)
        }
        .frame(width: 620, height: 480)
        .background(Theme.bgSoft)
    }

    private var logPanel: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    ForEach(Array(logs.enumerated()), id: \.offset) { i, line in
                        Text(line).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading).id(i)
                    }
                }
                .padding(10)
            }
            .frame(height: 150)
            .background(Theme.bg)
            .overlay(Rectangle().fill(Theme.borderSoft).frame(height: 1), alignment: .top)
            .overlay(Rectangle().fill(Theme.borderSoft).frame(height: 1), alignment: .bottom)
            .onChange(of: logs.count) { if let last = logs.indices.last { proxy.scrollTo(last, anchor: .bottom) } }
        }
    }

    private func node(_ st: PipeStage) -> some View {
        VStack(spacing: 8) {
            ZStack {
                Circle().fill(bg(st.status)).frame(width: 44, height: 44)
                icon(st.status)
            }
            Text(st.title).font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.fg).lineLimit(1)
            Text(durationText(st)).font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
        }
        .frame(width: 140)
    }

    @ViewBuilder private func icon(_ s: StageStatus) -> some View {
        switch s {
        case .pending: Image(systemName: "circle").font(.system(size: 16)).foregroundStyle(Theme.dim)
        case .running: ProgressView().controlSize(.small)
        case .ok:      Image(systemName: "checkmark").font(.system(size: 18, weight: .bold)).foregroundStyle(.white)
        case .failed:  Image(systemName: "xmark").font(.system(size: 18, weight: .bold)).foregroundStyle(.white)
        case .skipped: Image(systemName: "minus").font(.system(size: 18, weight: .bold)).foregroundStyle(Theme.fgMuted)
        }
    }
    private func bg(_ s: StageStatus) -> Color {
        switch s {
        case .ok: return Theme.ok
        case .failed: return Theme.danger
        case .running: return Theme.accent.opacity(0.25)
        default: return Theme.hover
        }
    }
    private func durationText(_ st: PipeStage) -> String {
        switch st.status {
        case .running: return "running…"
        case .skipped: return "skipped"
        case .ok, .failed: return st.ms >= 1000 ? String(format: "%.1fs", Double(st.ms) / 1000) : "\(st.ms)ms"
        default: return "—"
        }
    }

    private func run() {
        running = true
        logs = []
        showLog = true
        for i in stages.indices { stages[i].status = .pending; stages[i].ms = 0 }
        streamID = StreamManager.shared.openPrepareMain(skipSeed: skipSeed) { kind, bytes in
            DispatchQueue.main.async {
                if kind == .close { running = false; finished = true; return }
                guard kind == .json, let ev = PomJSON.decode(PrepareEvent.self, from: Data(bytes)) else { return }
                if ev.status == "plan" {
                    let titles = ["reset": "Reset databases", "migrate": "Migrate", "seed": "Seed"]
                    stages = ev.detail.split(separator: ",").map { id in
                        PipeStage(id: String(id), title: titles[String(id)] ?? String(id))
                    }
                    return
                }
                if ev.status == "log" { logs.append(ev.detail); return }
                guard let idx = stages.firstIndex(where: { $0.id == ev.stage }) else { return }
                stages[idx].status = StageStatus(rawValue: ev.status) ?? .running
                stages[idx].ms = ev.ms
                stages[idx].detail = ev.detail
            }
        }
        if streamID <= 0 { running = false; finished = true }
    }
}
