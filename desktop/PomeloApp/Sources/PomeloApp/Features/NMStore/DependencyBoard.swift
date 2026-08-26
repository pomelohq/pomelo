import SwiftUI
import Grape

// Global (session-independent) dependency-cache board. A force-directed graph with
// three tiers: cache TYPE (node_modules today; go/bundle/venv later) -> repo -> the
// cached lockfile hashes for that repo. Current hash (matches main's lockfile) is
// green; stale ones that drifted are amber (tap to reclaim). Pinch to zoom, drag to
// pan. Uses the Grape SwiftUI graph library.
struct DependencyBoard: View {
    var onClose: () -> Void = {}
    @StateObject private var vm = NMStoreViewModel()
    @State private var graphStates = ForceDirectedGraphState(initialIsRunning: true)

    private enum Kind { case type, repo, hash }
    private struct GNode {
        let id: String
        let kind: Kind
        let color: Color
        let label: String
        let icon: String
        let size: CGFloat
    }

    private let typeID = "type:node_modules"

    private var nodes: [GNode] {
        var out: [GNode] = [
            GNode(id: typeID, kind: .type, color: Theme.accent,
                  label: "node_modules", icon: "shippingbox.fill", size: 26)
        ]
        let byRepo = Dictionary(grouping: vm.entries, by: \.repo)
        for repo in byRepo.keys.sorted() {
            out.append(GNode(id: "repo:\(repo)", kind: .repo, color: Theme.fg,
                             label: repo, icon: "folder.fill", size: 20))
            for e in byRepo[repo] ?? [] {
                out.append(GNode(id: "hash:\(e.repo)/\(e.hash)", kind: .hash,
                                 color: e.current ? Theme.ok : Theme.warn,
                                 label: "\(e.hash.prefix(7)) - \(vm.human(e.bytes))",
                                 icon: "internaldrive.fill", size: 16))
            }
        }
        return out
    }
    private var links: [(String, String)] {
        var out: [(String, String)] = []
        let byRepo = Dictionary(grouping: vm.entries, by: \.repo)
        for repo in byRepo.keys.sorted() {
            out.append((typeID, "repo:\(repo)"))
            for e in byRepo[repo] ?? [] {
                out.append(("repo:\(repo)", "hash:\(e.repo)/\(e.hash)"))
            }
        }
        return out
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "shippingbox.fill").font(.system(size: 13)).foregroundStyle(Theme.accent)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Dependency store").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text("node_modules cache - \(vm.human(vm.total)) total").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
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
                graph.overlay(alignment: .bottomLeading) { hint }
            }
        }
        .frame(width: 860, height: 620).background(Theme.bg)
        .task { await vm.load() }
    }

    private var hint: some View {
        Text("pinch to zoom - drag to pan - tap a stale cache to reclaim")
            .font(.system(size: 10)).foregroundStyle(Theme.dim)
            .padding(.horizontal, 14).padding(.vertical, 10)
    }

    // Icon + name as a single NATIVE text annotation (Grape draws it in the Canvas).
    // ViewAnnotations (AnyView) re-render every simulation tick and make dragging lag.
    private func nodeText(_ n: GNode) -> Text {
        Text(Image(systemName: n.icon)).font(.system(size: n.size)).foregroundColor(n.color)
        + Text(verbatim: "\n\(n.label)").font(.system(size: 9)).foregroundColor(Theme.fgMuted)
    }

    private var graph: some View {
        ForceDirectedGraph(states: graphStates) {
            Series(nodes) { n in
                NodeMark(id: n.id)
                    .symbolSize(radius: 3)
                    .foregroundStyle(n.color)
                    .annotation(nodeText(n), alignment: .center, offset: .zero)
            }
            Series(links) { from, to in
                LinkMark(from: from, to: to)
            }
        } force: {
            .manyBody(strength: -80.0)
            .link(originalLength: 62.0)
            .center()
        }
        .graphOverlay { proxy in
            Rectangle().fill(.clear).contentShape(Rectangle())
                .withGraphMagnifyGesture(proxy)
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
