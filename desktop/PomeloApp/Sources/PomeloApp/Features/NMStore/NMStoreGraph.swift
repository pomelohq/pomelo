import SwiftUI

// Dependency store as a small node graph: per repo, a center "main lockfile" node
// with edges to each cached node_modules hash — the current one (matches main,
// green/solid) vs stale ones that drifted (amber/dashed, reclaimable).
struct NMStoreGraph: View {
    @ObservedObject var vm: NMStoreViewModel

    private var repos: [(repo: String, entries: [NMStoreViewModel.Entry])] {
        Dictionary(grouping: vm.entries, by: \.repo)
            .map { ($0.key, $0.value.sorted { ($0.current ? 1 : 0, $0.bytes) > ($1.current ? 1 : 0, $1.bytes) }) }
            .sorted { $0.repo < $1.repo }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                Text("Dependency store").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.fg)
                if vm.total > 0 {
                    Text(vm.human(vm.total) + " total").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                }
                Spacer()
                if !vm.stale.isEmpty {
                    Button { Task { await vm.deleteStale() } } label: {
                        Label("Reclaim \(vm.human(vm.staleBytes))", systemImage: "trash")
                            .font(.system(size: 11, weight: .medium))
                    }.buttonStyle(.plain).foregroundStyle(Theme.danger)
                }
            }
            if vm.loading {
                HStack(spacing: 8) { ProgressView().controlSize(.small); Text("scanning…").foregroundStyle(Theme.fgMuted) }
            } else if repos.isEmpty {
                Text("No cached node_modules yet.").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
            } else {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(alignment: .top, spacing: 28) {
                        ForEach(repos, id: \.repo) { RepoCluster(repo: $0.repo, entries: $0.entries, vm: vm) }
                    }.padding(.vertical, 4)
                }
            }
            Text("Cached node_modules are cloned into new workspaces instead of a full install. The green node matches main's lockfile; amber nodes drifted and are safe to delete — the next workspace reinstalls and re-caches.")
                .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
        }
        .padding(.vertical, 4)
        .task { await vm.load() }
    }
}

private struct RepoCluster: View {
    let repo: String
    let entries: [NMStoreViewModel.Entry]
    @ObservedObject var vm: NMStoreViewModel

    private let nodeW: CGFloat = 104
    private let gap: CGFloat = 14
    private let edgeH: CGFloat = 30

    private var width: CGFloat { CGFloat(entries.count) * nodeW + CGFloat(max(0, entries.count - 1)) * gap }

    var body: some View {
        VStack(spacing: 0) {
            // center "main lockfile" node
            VStack(spacing: 1) {
                Text(repo).font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.fg).lineLimit(1)
                Text("main lockfile").font(.system(size: 9)).foregroundStyle(Theme.dim)
            }
            .padding(.horizontal, 10).padding(.vertical, 5)
            .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 7))
            .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.border))

            // edges
            Canvas { ctx, size in
                let topX = size.width / 2
                for (i, e) in entries.enumerated() {
                    let childX = CGFloat(i) * (nodeW + gap) + nodeW / 2
                    var p = Path(); p.move(to: CGPoint(x: topX, y: 0)); p.addLine(to: CGPoint(x: childX, y: size.height))
                    let color = e.current ? Theme.ok : Theme.warn
                    let style = e.current
                        ? StrokeStyle(lineWidth: 1.5)
                        : StrokeStyle(lineWidth: 1.2, dash: [3, 3])
                    ctx.stroke(p, with: .color(color.opacity(0.7)), style: style)
                }
            }
            .frame(width: width, height: edgeH)

            // cache-hash nodes
            HStack(alignment: .top, spacing: gap) {
                ForEach(entries) { node($0) }
            }
        }
        .frame(width: width)
    }

    private func node(_ e: NMStoreViewModel.Entry) -> some View {
        let color = e.current ? Theme.ok : Theme.warn
        return VStack(spacing: 2) {
            Text(String(e.hash.prefix(7))).font(Theme.mono(10.5)).foregroundStyle(Theme.fg)
            Text(vm.human(e.bytes)).font(Theme.mono(9.5)).foregroundStyle(Theme.fgMuted)
            if e.current {
                Text("in use").font(.system(size: 8.5, weight: .medium)).foregroundStyle(Theme.ok)
            } else {
                Button { Task { await vm.delete(e) } } label: {
                    HStack(spacing: 2) { Image(systemName: "xmark").font(.system(size: 7)); Text("reclaim").font(.system(size: 8.5, weight: .medium)) }
                        .foregroundStyle(Theme.warn)
                }.buttonStyle(.plain)
            }
        }
        .frame(width: nodeW).padding(.vertical, 6)
        .background(color.opacity(0.10), in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(color.opacity(e.current ? 0.5 : 0.35),
                                                                 style: StrokeStyle(lineWidth: 1, dash: e.current ? [] : [3, 3])))
    }
}
