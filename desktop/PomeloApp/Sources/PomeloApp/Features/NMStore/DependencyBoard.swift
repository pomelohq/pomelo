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

    private enum Kind { case type, hash, workspace }
    private struct GNode {
        let id: String
        let kind: Kind
        let color: Color
        let label: String
        let icon: String
        let size: CGFloat
    }

    private let typeID = "type:node_modules"

    // Tiers: node_modules -> each cached hash (labelled by repo, red if no workspace
    // uses it = reclaimable) -> the workspaces currently resolving to that hash.
    private var nodes: [GNode] {
        var out: [GNode] = [
            GNode(id: typeID, kind: .type, color: Theme.accent,
                  label: "node_modules", icon: "shippingbox.fill", size: 22)
        ]
        var seenWs = Set<String>()
        for e in vm.entries {
            let hid = "hash:\(e.repo)/\(e.hash)"
            out.append(GNode(id: hid, kind: .hash,
                             color: e.orphan ? Theme.danger : Theme.ok,
                             label: "\(e.repo)  \(e.hash.prefix(7)) - \(vm.human(e.bytes))",
                             icon: "internaldrive.fill", size: 15))
            for c in e.consumers {
                let wid = "ws:\(c.branch)"
                if seenWs.insert(wid).inserted {
                    out.append(GNode(id: wid, kind: .workspace,
                                     color: c.is_main ? Theme.accent : Theme.fg,
                                     label: c.branch, icon: c.is_main ? "star.fill" : "square.stack.3d.up.fill",
                                     size: 13))
                }
            }
        }
        return out
    }
    private var links: [(String, String)] {
        var out: [(String, String)] = []
        for e in vm.entries {
            let hid = "hash:\(e.repo)/\(e.hash)"
            out.append((typeID, hid))
            for c in e.consumers {
                out.append((hid, "ws:\(c.branch)"))
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
        Text("each cache links to the workspaces using it - red = unused (tap to reclaim) - drag to arrange, pinch to zoom")
            .font(.system(size: 10)).foregroundStyle(Theme.dim)
            .padding(.horizontal, 14).padding(.vertical, 10)
    }

    private var graph: some View {
        ForceDirectedGraph(states: graphStates) {
            Series(nodes) { n in
                // The icon sits exactly on the node (alignment .center) so links
                // connect to it; the mark itself is an invisible hit target. The name
                // is a cheap native text below.
                NodeMark(id: n.id)
                    .symbolSize(radius: 4)
                    .foregroundStyle(.clear)
                    .annotation(n.id, alignment: .center, offset: .zero) {
                        Image(systemName: n.icon).font(.system(size: n.size)).foregroundStyle(n.color)
                    }
                    .annotation(Text(verbatim: n.label).font(.system(size: 10)).foregroundColor(Theme.fgMuted),
                                alignment: .bottom, offset: CGVector(dx: 0, dy: CGFloat(n.size) + 10))
            }
            Series(links) { from, to in
                LinkMark(from: from, to: to)
            }
        } force: {
            .manyBody(strength: -80.0)
            .link(originalLength: 62.0)
            .center()
        }
        .graphBackground { _ in GridBackground() }
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
        if let e = vm.entries.first(where: { "\($0.repo)/\($0.hash)" == key }), e.orphan {
            Task { await vm.delete(e) }
        }
    }
}

private struct GridBackground: View {
    var body: some View {
        Canvas { ctx, size in
            let step: CGFloat = 26
            var path = Path()
            var x: CGFloat = 0
            while x <= size.width { path.move(to: CGPoint(x: x, y: 0)); path.addLine(to: CGPoint(x: x, y: size.height)); x += step }
            var y: CGFloat = 0
            while y <= size.height { path.move(to: CGPoint(x: 0, y: y)); path.addLine(to: CGPoint(x: size.width, y: y)); y += step }
            ctx.stroke(path, with: .color(Theme.borderSoft.opacity(0.45)), lineWidth: 0.5)
        }
        .background(Theme.bg)
    }
}
