import SwiftUI

struct SyncSection: View {
    @Environment(AppState.self) var state
    @StateObject private var vm = SyncViewModel()

    private func apply() async { await vm.save(); await state.refreshSync() }

    var body: some View {
        Section {
            Toggle("Keep main fresh", isOn: $vm.refreshMain)
                .onChange(of: vm.refreshMain) { if vm.loaded { Task { await apply() } } }
            if vm.refreshMain {
                HStack(spacing: 6) {
                    Text("Every")
                    MinuteSelect(value: $vm.intervalMin) { if vm.loaded { Task { await apply() } } }
                    Text("min · \(clockHint(vm.intervalMin))").foregroundStyle(.secondary)
                    Spacer()
                }
            }
        } header: { Text("Golden source") } footer: {
            Text("Periodically git pull --ff-only + migrate every repo in main so new workspaces clone an up-to-date node_modules and seeded DBs. Runs in the background.")
        }
        .task { await vm.load() }
    }

    private func clockHint(_ n: Int) -> String {
        if 60 % n == 0 {
            let mins = stride(from: 0, to: 60, by: n).map { String(format: ":%02d", $0) }
            if mins.count <= 4 { return "at " + mins.joined(separator: " ") }
        }
        return "on the clock"
    }
}

private struct MinuteSelect: View {
    @Binding var value: Int
    var onChange: () -> Void
    @State private var open = false
    @State private var hovering = false

    var body: some View {
        Button { open.toggle() } label: {
            HStack(spacing: 5) {
                Text("\(value)").font(Theme.mono(12, .medium)).foregroundStyle(Theme.fg)
                Text("min").font(.system(size: 11)).foregroundStyle(Theme.dim)
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(Theme.fgMuted)
            }
            .padding(.horizontal, 9).padding(.vertical, 4)
            .background((hovering || open) ? Theme.hover : Theme.bg, in: RoundedRectangle(cornerRadius: 7))
            .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(open ? Theme.accent.opacity(0.6) : Theme.border))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .popover(isPresented: $open, arrowEdge: .bottom) {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 2) {
                        ForEach(1...59, id: \.self) { n in
                            MinuteRow(n: n, selected: n == value) { value = n; open = false; onChange() }.id(n)
                        }
                    }
                    .padding(6)
                }
                .frame(width: 128, height: 224)
                .background(Theme.panel3)
                .onAppear { proxy.scrollTo(value, anchor: .center) }
            }
        }
    }
}

private struct MinuteRow: View {
    let n: Int
    let selected: Bool
    var pick: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: pick) {
            HStack(spacing: 6) {
                Text("\(n)").font(Theme.mono(12)).foregroundStyle(selected ? Theme.accent : Theme.fg)
                Text("min").font(.system(size: 10.5)).foregroundStyle(Theme.dim)
                Spacer(minLength: 8)
                if selected { Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.accent) }
            }
            .padding(.horizontal, 9).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(selected ? Theme.sel : (hovering ? Theme.hover : .clear), in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}
