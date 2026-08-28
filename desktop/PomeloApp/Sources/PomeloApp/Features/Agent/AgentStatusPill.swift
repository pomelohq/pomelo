import SwiftUI

struct AgentStatusPill: View {
    @EnvironmentObject var state: AppState
    @State private var open = false
    @State private var hovered: String?

    private static let priority = ["awaiting_input", "thinking", "tool_use", "compacting", "idle"]

    private var active: [(ws: Workspace, state: String)] {
        state.workspaces.compactMap { ws in
            guard let s = state.agentStates[ws.id] ?? ws.claudeAgentState, Self.priority.contains(s) else { return nil }
            return (ws, s)
        }
        .sorted { rank($0.state) < rank($1.state) }
    }

    private func rank(_ s: String) -> Int { Self.priority.firstIndex(of: s) ?? Self.priority.count }
    private func busy(_ s: String) -> Bool { s != "idle" }

    private func color(_ s: String) -> Color {
        switch s {
        case "idle":           return Theme.ok
        case "thinking":       return Theme.warn
        case "tool_use":       return Theme.tool
        case "compacting":     return Theme.wsAccent
        case "awaiting_input": return Theme.danger
        default:               return Theme.dim
        }
    }

    private func label(_ s: String) -> String {
        switch s {
        case "idle":           return "Idle"
        case "thinking":       return "Thinking"
        case "tool_use":       return "Using tools"
        case "compacting":     return "Compacting"
        case "awaiting_input": return "Awaiting input"
        default:               return s
        }
    }

    var body: some View {
        let items = active
        if !items.isEmpty {
            let top = items[0].state
            Button { open.toggle() } label: {
                HStack(spacing: 5) {
                    AgentOrb(color: color(top), active: items.contains { busy($0.state) })
                    Text("\(items.count)").font(Theme.mono(10.5)).foregroundStyle(Theme.fg)
                }
                .padding(.horizontal, 7).padding(.vertical, 3)
                .background(open ? Theme.hover : Theme.chip, in: Capsule())
                .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .dropdownMenu(isPresented: $open, alignment: .top, drop: 24) { detail(items) }
        }
    }

    private func detail(_ items: [(ws: Workspace, state: String)]) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("Agents").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.fgMuted)
                .padding(.horizontal, 8).padding(.top, 4).padding(.bottom, 2)
            ForEach(items, id: \.ws.id) { item in
                Button { state.selection = item.ws.id; open = false } label: {
                    HStack(spacing: 8) {
                        Circle().fill(color(item.state)).frame(width: 7, height: 7)
                        Text(item.ws.title).font(.system(size: 12)).foregroundStyle(Theme.fg).lineLimit(1)
                        Spacer(minLength: 12)
                        Text(label(item.state)).font(Theme.mono(9.5)).foregroundStyle(Theme.dim)
                    }
                    .padding(.horizontal, 8).padding(.vertical, 5)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(hovered == item.ws.id ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 6))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .onHover { hovered = $0 ? item.ws.id : (hovered == item.ws.id ? nil : hovered) }
            }
        }
        .padding(4).frame(width: 260)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.border))
    }
}
