import SwiftUI
import WidgetKit

struct AgentEntry: TimelineEntry {
    let date: Date
    let snapshot: AgentSnapshot?
}

struct AgentProvider: TimelineProvider {
    func placeholder(in context: Context) -> AgentEntry {
        AgentEntry(date: Date(), snapshot: nil)
    }
    func getSnapshot(in context: Context, completion: @escaping (AgentEntry) -> Void) {
        completion(AgentEntry(date: Date(), snapshot: SharedStore.load()))
    }
    func getTimeline(in context: Context, completion: @escaping (Timeline<AgentEntry>) -> Void) {
        let entry = AgentEntry(date: Date(), snapshot: SharedStore.load())
        completion(Timeline(entries: [entry], policy: .after(Date().addingTimeInterval(600))))
    }
}

struct PomeloAgentWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "PomeloAgentWidget", provider: AgentProvider()) { entry in
            AgentWidgetView(entry: entry)
                .containerBackground(for: .widget) { Color(.systemBackground) }
        }
        .configurationDisplayName("Agents")
        .description("Live status of your Pomelo agents.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

private struct AgentWidgetView: View {
    @Environment(\.widgetFamily) var family
    let entry: AgentEntry

    private var active: [AgentWorkspaceLite] {
        (entry.snapshot?.workspaces ?? [])
            .filter { agentStateActive($0.state) }
            .sorted { rank($0.state) < rank($1.state) }
    }
    private func rank(_ s: String) -> Int {
        s == "awaiting_input" ? 0 : 1
    }

    var body: some View {
        if let snap = entry.snapshot {
            content(snap)
        } else {
            VStack(spacing: 6) {
                Image(systemName: "circle.dashed").font(.title2).foregroundStyle(.secondary)
                Text("Open Pomelo to sync").font(.caption2).foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
        }
    }

    @ViewBuilder private func content(_ snap: AgentSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text("Agents").font(.system(size: 13, weight: .semibold))
                Spacer()
                Text("\(snap.activeCount)").font(.system(size: 15, weight: .bold).monospacedDigit())
                    .foregroundStyle(snap.activeCount > 0 ? .primary : .secondary)
            }
            let rows = active
            if rows.isEmpty {
                Spacer(minLength: 0)
                Text("All idle").font(.caption).foregroundStyle(.secondary)
                Spacer(minLength: 0)
            } else {
                let limit = family == .systemSmall ? 3 : 5
                ForEach(rows.prefix(limit)) { ws in
                    HStack(spacing: 7) {
                        Circle().fill(agentStateColor(ws.state)).frame(width: 8, height: 8)
                        Text(ws.title).font(.system(size: 12)).lineLimit(1)
                        Spacer(minLength: 0)
                        if family != .systemSmall {
                            Text(agentStateLabel(ws.state)).font(.system(size: 10, weight: .medium))
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                if rows.count > limit {
                    Text("+\(rows.count - limit) more").font(.system(size: 10)).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
