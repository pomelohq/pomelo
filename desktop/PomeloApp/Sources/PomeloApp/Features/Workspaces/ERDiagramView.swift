import SwiftUI

private struct ERFrameKey: PreferenceKey {
    static var defaultValue: [String: CGRect] = [:]
    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) {
        value.merge(nextValue(), uniquingKeysWith: { _, b in b })
    }
}

struct ERDiagramView: View {
    let model: ReviewModel
    var onOpenEntity: (ModelEntity) -> Void
    @State private var frames: [String: CGRect] = [:]

    private var changedColor: Color { Theme.ok }
    private let columns = [GridItem(.adaptive(minimum: 240, maximum: 340), spacing: 40, alignment: .top)]

    var body: some View {
        VStack(spacing: 0) {
            legend
            Divider().overlay(Theme.borderSoft)
            ScrollView([.vertical, .horizontal]) {
                ZStack(alignment: .topLeading) {
                    Canvas { ctx, _ in
                        for rel in model.relations {
                            guard let a = frames[rel.from], let b = frames[rel.to] else { continue }
                            drawEdge(ctx, a, b, rel)
                        }
                    }
                    .opacity(frames.isEmpty ? 0 : 1)
                    .animation(.easeInOut(duration: 0.25), value: frames.isEmpty)
                    LazyVGrid(columns: columns, alignment: .leading, spacing: 40) {
                        ForEach(model.entities) { e in
                            entityBox(e)
                                .background(GeometryReader { g in
                                    Color.clear.preference(key: ERFrameKey.self, value: [e.id: g.frame(in: .named("er"))])
                                })
                        }
                    }
                    .padding(28)
                }
                .coordinateSpace(name: "er")
                .onPreferenceChange(ERFrameKey.self) { frames = $0 }
            }
        }
        .background(Theme.bg)
    }

    private var legend: some View {
        HStack(spacing: 14) {
            badgeChip("PK", Theme.warn); Text("primary key").font(.system(size: 11)).foregroundStyle(Theme.dim)
            badgeChip("FK", Theme.tool); Text("foreign key").font(.system(size: 11)).foregroundStyle(Theme.dim)
            HStack(spacing: 5) {
                RoundedRectangle(cornerRadius: 2).fill(changedColor.opacity(0.5)).frame(width: 10, height: 10)
                Text("added / changed by this branch").font(.system(size: 11)).foregroundStyle(Theme.dim)
            }
            Spacer()
        }
        .padding(.horizontal, 16).padding(.vertical, 8)
        .background(Theme.bgSoft)
    }

    private func badgeChip(_ t: String, _ c: Color) -> some View {
        Text(t).font(Theme.mono(8, .bold)).foregroundStyle(c)
            .padding(.horizontal, 3).padding(.vertical, 1)
            .background(c.opacity(0.16), in: RoundedRectangle(cornerRadius: 3))
    }

    private func drawEdge(_ ctx: GraphicsContext, _ a: CGRect, _ b: CGRect, _ rel: ModelRelation) {
        let p0 = edgePoint(from: a, toward: b.center)
        let p1 = edgePoint(from: b, toward: a.center)
        var path = Path()
        path.move(to: p0)
        let midX = (p0.x + p1.x) / 2, midY = (p0.y + p1.y) / 2
        path.addCurve(to: p1, control1: CGPoint(x: midX, y: p0.y), control2: CGPoint(x: midX, y: p1.y))
        ctx.stroke(path, with: .color(Theme.fgMuted.opacity(0.7)), lineWidth: 1.5)
        for pt in [p0, p1] {
            ctx.fill(Path(ellipseIn: CGRect(x: pt.x - 2.5, y: pt.y - 2.5, width: 5, height: 5)), with: .color(Theme.fgMuted.opacity(0.7)))
        }
        let text = [rel.kind, rel.label].filter { !$0.isEmpty }.joined(separator: "  ")
        guard !text.isEmpty else { return }
        let mid = CGPoint(x: midX, y: midY)
        let resolved = ctx.resolve(Text(text).font(Theme.mono(9.5, .medium)).foregroundColor(Theme.fgMuted))
        let sz = resolved.measure(in: CGSize(width: 260, height: 40))
        let rect = CGRect(x: mid.x - sz.width / 2 - 5, y: mid.y - sz.height / 2 - 2, width: sz.width + 10, height: sz.height + 4)
        ctx.fill(Path(roundedRect: rect, cornerRadius: 4), with: .color(Theme.panel3))
        ctx.stroke(Path(roundedRect: rect, cornerRadius: 4), with: .color(Theme.borderSoft), lineWidth: 0.75)
        ctx.draw(resolved, at: mid)
    }

    private func edgePoint(from rect: CGRect, toward pt: CGPoint) -> CGPoint {
        let c = rect.center
        let dx = pt.x - c.x, dy = pt.y - c.y
        guard dx != 0 || dy != 0 else { return c }
        let hw = rect.width / 2, hh = rect.height / 2
        let sx = dx == 0 ? .greatestFiniteMagnitude : hw / abs(dx)
        let sy = dy == 0 ? .greatestFiniteMagnitude : hh / abs(dy)
        let s = min(sx, sy)
        return CGPoint(x: c.x + dx * s, y: c.y + dy * s)
    }

    private func entityBox(_ e: ModelEntity) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Button { onOpenEntity(e) } label: {
                HStack(spacing: 6) {
                    Text(e.label).font(Theme.mono(12.5, .semibold)).foregroundStyle(Theme.fg).lineLimit(1)
                    if !e.repo.isEmpty {
                        Text(e.repo).font(Theme.mono(9.5)).foregroundStyle(Theme.dim).lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    if e.hasCode { Image(systemName: "chevron.right").font(.system(size: 8, weight: .semibold)).foregroundStyle(Theme.dim) }
                }
                .padding(.horizontal, 10).padding(.vertical, 7)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background((e.changed ? changedColor.opacity(0.16) : Theme.panel3), in: UnevenRoundedRectangle(topLeadingRadius: 8, topTrailingRadius: 8))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(e.note.isEmpty ? (e.hasCode ? "Open definition" : "") : e.note)

            if !e.fields.isEmpty {
                Divider().overlay(Theme.borderSoft)
                VStack(spacing: 0) {
                    ForEach(e.fields) { f in fieldRow(f) }
                }
            }
        }
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(e.changed ? changedColor.opacity(0.55) : Theme.borderSoft))
    }

    private func fieldRow(_ f: ModelField) -> some View {
        HStack(spacing: 6) {
            keyBadge(f.key)
            Text(f.name).font(Theme.mono(11)).foregroundStyle(f.changed ? changedColor : Theme.fgSoft).lineLimit(1)
            Spacer(minLength: 8)
            if !f.type.isEmpty { Text(f.type).font(Theme.mono(10)).foregroundStyle(Theme.dim).lineLimit(1) }
        }
        .padding(.horizontal, 10).padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(f.changed ? changedColor.opacity(0.10) : .clear)
        .help(f.note)
    }

    @ViewBuilder private func keyBadge(_ key: String) -> some View {
        if key == "pk" || key == "fk" {
            Text(key.uppercased()).font(Theme.mono(8, .bold))
                .foregroundStyle(key == "pk" ? Theme.warn : Theme.tool)
                .padding(.horizontal, 3).padding(.vertical, 1)
                .background((key == "pk" ? Theme.warn : Theme.tool).opacity(0.16), in: RoundedRectangle(cornerRadius: 3))
                .frame(width: 22, alignment: .leading)
        } else {
            Color.clear.frame(width: 22, height: 1)
        }
    }
}

private extension CGRect {
    var center: CGPoint { CGPoint(x: midX, y: midY) }
}
