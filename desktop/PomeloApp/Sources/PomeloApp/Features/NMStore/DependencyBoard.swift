import SwiftUI

// Global (session-independent) dependency-cache board. Deterministic left-to-right
// layered layout (like Turbo/dagre, not a force sim — force layouts turn this
// bipartite data into crossing spaghetti and can't pan smoothly): node_modules ->
// each cached hash (red if unused = reclaimable) -> the workspaces using it.
// Drag to pan, pinch/buttons to zoom; hovering a node lights up its whole subtree.
struct DependencyBoard: View {
    var onClose: () -> Void = {}
    @StateObject private var vm = NMStoreViewModel()
    @State private var zoom: CGFloat = 1
    @GestureState private var pinch: CGFloat = 1
    @State private var pan: CGSize = .zero
    @GestureState private var dragPan: CGSize = .zero
    @State private var hovered: String?

    private enum Kind { case type, hash, workspace }
    private struct Node: Identifiable {
        let id: String
        let kind: Kind
        let icon: String
        let color: Color
        let label: String
        let x: CGFloat
        let y: CGFloat
        var entry: NMStoreViewModel.Entry? = nil
    }
    private struct Edge { let from: String; let to: String; let p1: CGPoint; let p2: CGPoint }

    private let pillW: CGFloat = 250
    private let pillH: CGFloat = 32
    private let rowGap: CGFloat = 46
    private let colGap: CGFloat = 330
    private let pad: CGFloat = 34

    private struct Layout { var nodes: [Node]; var edges: [Edge]; var size: CGSize }

    private func build() -> Layout {
        let hashes = vm.entries
        var wsMain: [String: Bool] = [:]
        var wsRows: [String: [Int]] = [:]
        for (i, e) in hashes.enumerated() {
            for c in e.consumers {
                wsMain[c.branch] = (wsMain[c.branch] ?? false) || c.is_main
                wsRows[c.branch, default: []].append(i)
            }
        }
        let wsList = wsRows.keys.sorted {
            let a = wsRows[$0]!, b = wsRows[$1]!
            let ba = Double(a.reduce(0, +)) / Double(a.count)
            let bb = Double(b.reduce(0, +)) / Double(b.count)
            return ba != bb ? ba < bb : $0 < $1
        }
        let wsIndex = Dictionary(uniqueKeysWithValues: wsList.enumerated().map { ($1, $0) })

        let rows = max(1, hashes.count, wsList.count)
        let contentH = pad * 2 + CGFloat(rows) * rowGap
        let xRoot = pad, xHash = pad + colGap, xWs = pad + colGap * 2
        let contentW = xWs + pillW + pad

        func y(_ i: Int, _ count: Int) -> CGFloat {
            let start = (contentH - CGFloat(count) * rowGap) / 2 + rowGap / 2
            return start + CGFloat(i) * rowGap
        }
        let rootY = contentH / 2

        var nodes: [Node] = [
            Node(id: "root", kind: .type, icon: "shippingbox.fill", color: Theme.accent,
                 label: "node_modules", x: xRoot, y: rootY)
        ]
        var edges: [Edge] = []
        let rootRight = CGPoint(x: xRoot + pillW, y: rootY)

        for (i, e) in hashes.enumerated() {
            let hy = y(i, hashes.count)
            let hid = "hash:\(e.repo)/\(e.hash)"
            nodes.append(Node(id: hid, kind: .hash, icon: "internaldrive.fill",
                              color: e.orphan ? Theme.danger : Theme.ok,
                              label: "\(e.repo)  \(e.hash.prefix(7)) - \(vm.human(e.bytes))",
                              x: xHash, y: hy, entry: e))
            edges.append(Edge(from: "root", to: hid, p1: rootRight, p2: CGPoint(x: xHash, y: hy)))
            for c in e.consumers {
                guard let j = wsIndex[c.branch] else { continue }
                edges.append(Edge(from: hid, to: "ws:\(c.branch)",
                                  p1: CGPoint(x: xHash + pillW, y: hy),
                                  p2: CGPoint(x: xWs, y: y(j, wsList.count))))
            }
        }
        for (j, b) in wsList.enumerated() {
            let main = wsMain[b] ?? false
            nodes.append(Node(id: "ws:\(b)", kind: .workspace,
                              icon: main ? "star.fill" : "square.stack.3d.up.fill",
                              color: main ? Theme.accent : Theme.fg, label: b,
                              x: xWs, y: y(j, wsList.count)))
        }
        return Layout(nodes: nodes, edges: edges, size: CGSize(width: contentW, height: contentH))
    }

    private func activeIDs(_ l: Layout) -> Set<String> {
        guard let h = hovered else { return [] }
        var set: Set<String> = [h]
        for e in l.edges where e.from == h || e.to == h { set.insert(e.from); set.insert(e.to) }
        return set
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.borderSoft)
            if vm.loading {
                VStack(spacing: 8) { ProgressView().controlSize(.small); Text("scanning…").foregroundStyle(Theme.fgMuted) }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if vm.entries.isEmpty {
                Text("No cached node_modules yet.").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                board.overlay(alignment: .bottomLeading) { hint }
            }
        }
        .frame(width: 900, height: 640).background(Theme.bg)
        .task { await vm.load() }
    }

    private var header: some View {
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
            zoomControls
            Button { onClose() } label: { Image(systemName: "xmark").font(.system(size: 12)) }
                .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).padding(.leading, 6)
        }
        .padding(.horizontal, 18).padding(.vertical, 12)
    }

    private var zoomControls: some View {
        HStack(spacing: 2) {
            Button { zoom = max(0.4, zoom - 0.15) } label: { Image(systemName: "minus.magnifyingglass") }
            Button { zoom = 1; pan = .zero } label: { Text("\(Int(zoom * 100))%").font(Theme.mono(10)).frame(width: 34) }
            Button { zoom = min(2, zoom + 0.15) } label: { Image(systemName: "plus.magnifyingglass") }
        }
        .buttonStyle(.plain).font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
        .padding(.horizontal, 6).padding(.vertical, 3)
        .background(Theme.chip, in: Capsule())
    }

    private var hint: some View {
        Text("hover a node to trace it - red = unused (tap to reclaim) - drag to pan, pinch or +/- to zoom")
            .font(.system(size: 10)).foregroundStyle(Theme.dim)
            .padding(.horizontal, 14).padding(.vertical, 10)
    }

    private var board: some View {
        let l = build()
        let s = zoom * pinch
        let active = activeIDs(l)
        return canvas(l, active: active)
            .scaleEffect(s, anchor: .topLeading)
            .frame(width: l.size.width * s, height: l.size.height * s, alignment: .topLeading)
            .offset(x: pan.width + dragPan.width, y: pan.height + dragPan.height)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .padding(16)
            .contentShape(Rectangle())
            .clipped()
            .gesture(
                DragGesture(minimumDistance: 2)
                    .updating($dragPan) { v, st, _ in st = v.translation }
                    .onEnded { v in pan.width += v.translation.width; pan.height += v.translation.height }
            )
            .simultaneousGesture(
                MagnificationGesture().updating($pinch) { v, st, _ in st = v }
                    .onEnded { v in zoom = min(2, max(0.4, zoom * v)) }
            )
    }

    private func canvas(_ l: Layout, active: Set<String>) -> some View {
        ZStack(alignment: .topLeading) {
            GridBackground()
            Canvas { ctx, _ in
                for e in l.edges {
                    let on = hovered != nil && (e.from == hovered || e.to == hovered)
                    let shade: Color = hovered == nil ? Theme.borderSoft.opacity(0.7)
                        : (on ? Theme.accent : Theme.borderSoft.opacity(0.18))
                    var p = Path()
                    p.move(to: e.p1)
                    let mx = (e.p1.x + e.p2.x) / 2
                    p.addCurve(to: e.p2, control1: CGPoint(x: mx, y: e.p1.y), control2: CGPoint(x: mx, y: e.p2.y))
                    ctx.stroke(p, with: .color(shade), lineWidth: on ? 1.8 : 1)
                }
            }
            .frame(width: l.size.width, height: l.size.height)
            ForEach(l.nodes) { n in
                let dim = hovered != nil && !active.contains(n.id)
                pill(n).opacity(dim ? 0.3 : 1).position(x: n.x + pillW / 2, y: n.y)
            }
        }
        .frame(width: l.size.width, height: l.size.height, alignment: .topLeading)
    }

    private func pill(_ n: Node) -> some View {
        let lit = hovered == n.id
        return HStack(spacing: 7) {
            Image(systemName: n.icon).font(.system(size: 12)).foregroundStyle(n.color).frame(width: 16)
            Text(n.label).font(.system(size: 11)).foregroundStyle(Theme.fg)
                .lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .frame(width: pillW, height: pillH, alignment: .leading)
        .background(lit ? Theme.hover : Theme.surface, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(n.color.opacity(lit ? 0.9 : (n.kind == .type ? 0.6 : 0.4)), lineWidth: lit ? 1.5 : 1))
        .contentShape(Rectangle())
        .onHover { hovered = $0 ? n.id : (hovered == n.id ? nil : hovered) }
        .onTapGesture { if let e = n.entry, e.orphan { Task { await vm.delete(e) } } }
        .help(n.entry?.orphan == true ? "Unused cache - tap to reclaim \(vm.human(n.entry?.bytes ?? 0))" : n.label)
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
            ctx.stroke(path, with: .color(Theme.borderSoft.opacity(0.35)), lineWidth: 0.5)
        }
    }
}
