import SwiftUI
import Grape

// Global (session-independent) dependency-cache board. Organised by cache TYPE
// (node_modules today; go/bundle/venv later); each type renders as an interactive
// force-directed graph: a repo node linked to its cached hashes — the current one
// (green, matches main's lockfile) vs stale ones that drifted (amber, tap to
// reclaim). Uses the Grape SwiftUI graph library.
struct DependencyBoard: View {
    var onClose: () -> Void = {}
    @StateObject private var vm = NMStoreViewModel()
    @State private var graphStates = ForceDirectedGraphState(initialIsRunning: true)

    private struct GNode { let id: String; let color: Color; let label: String }

    private var nodes: [GNode] {
        var out: [GNode] = []
        let byRepo = Dictionary(grouping: vm.entries, by: \.repo)
        for repo in byRepo.keys.sorted() {
            out.append(GNode(id: "repo:\(repo)", color: Theme.accent, label: repo))
            for e in byRepo[repo] ?? [] {
                out.append(GNode(id: "hash:\(e.repo)/\(e.hash)",
                                 color: e.current ? Theme.ok : Theme.warn,
                                 label: "\(e.hash.prefix(7)) · \(vm.human(e.bytes))"))
            }
        }
        return out
    }
    private var links: [(String, String)] {
        vm.entries.map { ("repo:\($0.repo)", "hash:\($0.repo)/\($0.hash)") }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "shippingbox.fill").font(.system(size: 13)).foregroundStyle(Theme.accent)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Dependency store").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text("node_modules · \(vm.human(vm.total)) total").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                }
                Spacer()
                if !vm.stale.isEmpty {
                    Button { Task { await vm.deleteStale() } } label: {
                        Label("Reclaim \(vm.human(vm.staleBytes))", systemImage: "trash").font(.system(size: 11, weight: .medium))
                    }.buttonStyle(.plain).foregroundStyle(Theme.danger)
                }
                Button { onClose() } label: { Image(systemName: "xmark").font(.system(size: 12)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).padding(.leading, 6)
            }
            .padding(.horizontal, 18).padding(.vertical, 12)
            Divider().overlay(Theme.borderSoft)

            if vm.loading {
                VStack(spacing: 8) { ProgressView().controlSize(.small); Text("scanning…").foregroundStyle(Theme.fgMuted) }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if vm.entries.isEmpty {
                Text("No cached node_modules yet.").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                graph
            }
        }
        .frame(width: 860, height: 620).background(Theme.bg)
        .task { await vm.load() }
    }

    private var graph: some View {
        ForceDirectedGraph(states: graphStates) {
            Series(nodes) { n in
                NodeMark(id: n.id)
                    .foregroundStyle(n.color)
                    .stroke()
                    .annotation(n.id, offset: .zero) {
                        Text(n.label).font(.system(size: 9)).foregroundStyle(Theme.fgMuted)
                    }
            }
            Series(links) { from, to in
                LinkMark(from: from, to: to)
            }
        } force: {
            .manyBody(strength: -30.0)
            .link(originalLength: 42.0)
            .center()
        }
        .graphOverlay { proxy in
            Rectangle().fill(.clear).contentShape(Rectangle())
                .withGraphDragGesture(proxy, of: String.self)
                .withGraphTapGesture(proxy, of: String.self) { id in tap(id) }
        }
    }

    private func tap(_ id: String) {
        guard id.hasPrefix("hash:") else { return }
        let key = String(id.dropFirst(5))   // "<repo>/<hash>"
        if let e = vm.entries.first(where: { "\($0.repo)/\($0.hash)" == key }), !e.current {
            Task { await vm.delete(e) }
        }
    }
}
