import SwiftUI


struct WsOpStage: Identifiable { let id: Int; var name: String; var status: StageStatus = .pending; var detail = "" }

struct WsOp: Identifiable {
    let id = UUID()
    let kind: String          // "create" | "delete"
    let branch: String
    let displayName: String
    let repos: [String]
    var title: String
    var stages: [WsOpStage] = []
    var current = -1
    var status: StageStatus = .pending
    var error = ""
    var streamID: Int32 = 0
}

extension AppState {
    var opBranches: Set<String> {
        Set(ops.filter { $0.status == .pending || $0.status == .running }.map(\.branch))
    }

    private var maxConcurrentOps: Int { 1 }

    func startCreate(branch: String, repos: [String], displayName: String) {
        let b = branch.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !b.isEmpty else { return }
        ops.append(WsOp(kind: "create", branch: b, displayName: displayName, repos: repos,
                        title: displayName.isEmpty ? b : displayName))
        pumpOps()
    }

    func startDelete(_ ws: Workspace) {
        if selection == ws.id { selection = "main:" + (workspaces.first(where: \.isMain)?.branch ?? "main") }
        ops.append(WsOp(kind: "delete", branch: ws.branch, displayName: ws.title, repos: [], title: ws.title))
        pumpOps()
    }

    func dismissOp(_ id: UUID) { ops.removeAll { $0.id == id } }

    private func pumpOps() {
        guard ops.filter({ $0.status == .running }).count < maxConcurrentOps else { return }
        guard let i = ops.firstIndex(where: { $0.status == .pending }) else { return }
        ops[i].status = .running
        let op = ops[i], opID = op.id
        let onFrame: (StreamKind, [UInt8]) -> Void = { [weak self] kind, bytes in
            DispatchQueue.main.async { self?.onOpFrame(opID, kind, bytes) }
        }
        let sid = op.kind == "create"
            ? StreamManager.shared.openCreateWorkspace(branch: op.branch, repos: op.repos, onFrame: onFrame)
            : StreamManager.shared.openDeleteWorkspace(branch: op.branch, onFrame: onFrame)
        if let j = opIndex(opID) { ops[j].streamID = sid }
        if sid <= 0 { fail(opID, "could not start") }
    }

    private func opIndex(_ id: UUID) -> Int? { ops.firstIndex { $0.id == id } }

    private func fail(_ id: UUID, _ msg: String) {
        guard let i = opIndex(id) else { return }
        ops[i].status = .failed; ops[i].error = msg
        pumpOps()
    }

    private func onOpFrame(_ id: UUID, _ kind: StreamKind, _ bytes: [UInt8]) {
        guard let i = opIndex(id) else { return }
        if kind == .close {
            if ops[i].status == .running { ops[i].status = .ok; finalize(id) }
            return
        }
        guard kind == .json,
              let obj = try? JSONSerialization.jsonObject(with: Data(bytes)) as? [String: Any],
              let type = obj["type"] as? String else { return }
        switch type {
        case "pipeline-started":
            if let names = obj["stages"] as? [String] {
                ops[i].stages = names.enumerated().map { WsOpStage(id: $0.offset, name: $0.element) }
            } else if let total = obj["total"] as? Int {
                ops[i].stages = (0..<total).map { WsOpStage(id: $0, name: "step \($0 + 1)") }
            }
        case "stage-started":
            let ix = obj["index"] as? Int ?? 0
            ops[i].current = ix
            if let j = stageIndex(i, ix) {
                if let n = obj["name"] as? String, !n.isEmpty { ops[i].stages[j].name = n }
                ops[i].stages[j].status = .running
            }
        case "stage-completed":
            if let j = stageIndex(i, ops[i].current) { ops[i].stages[j].status = .ok }
        case "stage-skipped":
            if let j = stageIndex(i, obj["index"] as? Int ?? ops[i].current) { ops[i].stages[j].status = .skipped }
        case "stage-progress":
            if let j = stageIndex(i, ops[i].current), let d = obj["detail"] as? String { ops[i].stages[j].detail = d }
        case "pipeline-completed":
            for j in ops[i].stages.indices where ops[i].stages[j].status == .running { ops[i].stages[j].status = .ok }
            ops[i].status = .ok; finalize(id)
        case "pipeline-failed":
            let ix = obj["index"] as? Int ?? ops[i].current
            if let j = stageIndex(i, ix) { ops[i].stages[j].status = .failed }
            ops[i].error = (obj["error"] as? String) ?? "failed"
            ops[i].status = .failed
            pumpOps()
        default: break
        }
    }

    private func stageIndex(_ opIdx: Int, _ stageID: Int) -> Int? {
        ops[opIdx].stages.firstIndex { $0.id == stageID }
    }

    private func finalize(_ id: UUID) {
        guard let i = opIndex(id) else { return }
        let op = ops[i]
        Task { @MainActor in
            if op.kind == "create", !op.displayName.trimmingCharacters(in: .whitespaces).isEmpty {
                await WorkspaceOpsStore.rename(branch: op.branch, isMain: false, displayName: op.displayName)
            }
            await refresh()
            if op.kind == "create" { selection = "ws:" + op.branch }
            try? await Task.sleep(nanoseconds: 1_400_000_000)
            dismissOp(id)
            pumpOps()
        }
    }
}

struct OpsBar: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        if !state.ops.isEmpty {
            VStack(spacing: 4) {
                ForEach(state.ops) { op in OpRow(op: op) }
            }
            .padding(.horizontal, 8).padding(.vertical, 6)
        }
    }
}

private struct OpRow: View {
    @EnvironmentObject var state: AppState
    let op: WsOp
    @State private var expanded = false

    private var subtitle: String {
        switch op.status {
        case .pending: return "queued"
        case .failed:  return op.error.isEmpty ? "failed" : op.error
        case .ok:      return "done"
        default:
            if op.current >= 0, let s = op.stages.first(where: { $0.id == op.current }) { return s.name }
            return "starting…"
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            Button { expanded.toggle() } label: {
                HStack(spacing: 8) {
                    icon
                    VStack(alignment: .leading, spacing: 1) {
                        Text((op.kind == "delete" ? "Deleting " : "") + op.title)
                            .font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.fg).lineLimit(1)
                        Text(subtitle).font(.system(size: 10.5)).foregroundStyle(op.status == .failed ? Theme.danger : Theme.fgMuted).lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    if op.status == .failed { Button { state.dismissOp(op.id) } label: { Image(systemName: "xmark").font(.system(size: 9)) }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted) }
                    else { Image(systemName: expanded ? "chevron.up" : "chevron.down").font(.system(size: 9)).foregroundStyle(Theme.dim) }
                }
                .padding(.horizontal, 8).padding(.vertical, 6)
                .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 8))
            }.buttonStyle(.plain)
            if expanded, !op.stages.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(op.stages) { s in
                        HStack(alignment: .top, spacing: 7) {
                            stageIcon(s.status)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(s.name).font(.system(size: 11)).foregroundStyle(s.status == .pending ? Theme.dim : Theme.fgMuted).lineLimit(1)
                                if s.status == .running, !s.detail.isEmpty {
                                    Text(s.detail).font(Theme.mono(9.5)).foregroundStyle(Theme.dim)
                                        .lineLimit(1).truncationMode(.middle)
                                }
                            }
                            Spacer()
                        }
                    }
                }
                .padding(.horizontal, 10).padding(.vertical, 6)
            }
        }
    }

    @ViewBuilder private var icon: some View {
        switch op.status {
        case .running: ProgressView().controlSize(.small).scaleEffect(0.7).frame(width: 14)
        case .ok:      Image(systemName: "checkmark.circle.fill").font(.system(size: 12)).foregroundStyle(Theme.ok).frame(width: 14)
        case .failed:  Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 12)).foregroundStyle(Theme.danger).frame(width: 14)
        default:       Image(systemName: "clock").font(.system(size: 11)).foregroundStyle(Theme.dim).frame(width: 14)
        }
    }

    @ViewBuilder private func stageIcon(_ s: StageStatus) -> some View {
        switch s {
        case .running: ProgressView().controlSize(.small).scaleEffect(0.6).frame(width: 12)
        case .ok:      Image(systemName: "checkmark.circle.fill").font(.system(size: 10)).foregroundStyle(Theme.ok).frame(width: 12)
        case .failed:  Image(systemName: "xmark.circle.fill").font(.system(size: 10)).foregroundStyle(Theme.danger).frame(width: 12)
        case .skipped: Image(systemName: "minus.circle").font(.system(size: 10)).foregroundStyle(Theme.dim).frame(width: 12)
        case .pending: Image(systemName: "circle").font(.system(size: 9)).foregroundStyle(Theme.dim).frame(width: 12)
        }
    }
}
