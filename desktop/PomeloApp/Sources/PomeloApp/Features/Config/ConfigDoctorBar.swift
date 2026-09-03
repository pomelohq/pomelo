import SwiftUI

struct ConfigDoctorBar: View {
    @Environment(AppState.self) var state
    @ObservedObject var vm: DoctorViewModel
    @State private var expanded = false

    private var hasFindings: Bool { !vm.visibleFindings.isEmpty }

    var body: some View {
        VStack(spacing: 0) {
            if expanded && hasFindings {
                ScrollView {
                    VStack(spacing: 6) { ForEach(vm.visibleFindings) { finding($0) } }.padding(12)
                }
                .frame(maxHeight: 220).background(Theme.bg)
                Divider().overlay(Theme.borderSoft)
            }
            statusRow
        }
    }

    private var statusRow: some View {
        HStack(spacing: 9) {
            if vm.loading {
                Spinner(size: 12)
                Text("diagnosing…").font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
            } else {
                icon
                Text(summary).font(.system(size: 12, weight: .medium)).foregroundStyle(vm.healthy ? Theme.ok : Theme.danger)
                if hasFindings {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.dim)
                }
            }
            Spacer()
            if !vm.loading && !vm.healthy {
                Button { startFix() } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "sparkles").font(.system(size: 10))
                        Text("Fix with Claude").font(.system(size: 11.5, weight: .medium))
                    }
                    .foregroundStyle(Theme.accent)
                    .padding(.horizontal, 9).padding(.vertical, 3)
                    .background(Theme.accentSoft, in: Capsule())
                }.buttonStyle(.plain).disabled(state.agentRunning)
            }
            IconButton("arrow.clockwise", tip: "Re-run doctor") { Task { await vm.load() } }
        }
        .padding(.horizontal, 16).padding(.vertical, 9)
        .background(Theme.bgSoft)
        .contentShape(Rectangle())
        .onTapGesture { if hasFindings { expanded.toggle() } }
    }

    private func startFix() {
        state.launchAgent(
            title: "Fix",
            subtitle: "The Doctor reads the config, fixes the findings via the pom MCP tools, and re-runs config_doctor until clean.",
            runningLabel: "Fixing config — diagnose → fix → verify…",
            branch: "", role: "fixer", firstTurn: vm.fixPrompt())
    }

    private var summary: String {
        if vm.healthy { return "Ready to run — no blocking gaps" }
        let e = "\(vm.errors) error\(vm.errors == 1 ? "" : "s")"
        return vm.warnings > 0 ? "\(e) · \(vm.warnings) warning\(vm.warnings == 1 ? "" : "s")" : e
    }

    @ViewBuilder private var icon: some View {
        if vm.healthy {
            Image(systemName: "checkmark.seal.fill").font(.system(size: 12)).foregroundStyle(Theme.ok)
        } else {
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 12)).foregroundStyle(Theme.danger)
        }
    }

    private func finding(_ f: DoctorViewModel.Finding) -> some View {
        Card {
            HStack(alignment: .top, spacing: 9) {
                findingIcon(f.severity).frame(width: 14)
                VStack(alignment: .leading, spacing: 2) {
                    Text(f.title).font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.fg)
                    if !f.detail.isEmpty { Text(f.detail).font(Theme.mono(10)).foregroundStyle(Theme.dim).textSelection(.enabled) }
                    if !f.fix.isEmpty { Text("→ \(f.fix)").font(.system(size: 10.5)).foregroundStyle(Theme.fgMuted) }
                }
                Spacer(minLength: 0)
            }
            .padding(9).frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder private func findingIcon(_ sev: String) -> some View {
        switch sev {
        case "error": Image(systemName: "xmark.circle.fill").font(.system(size: 12)).foregroundStyle(Theme.danger)
        case "warn":  Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 11)).foregroundStyle(Theme.warn)
        default:      Image(systemName: "checkmark.circle.fill").font(.system(size: 12)).foregroundStyle(Theme.ok)
        }
    }
}
