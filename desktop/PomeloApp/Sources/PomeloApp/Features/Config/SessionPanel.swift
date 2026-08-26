import SwiftUI

struct SessionPanel: View {
    @EnvironmentObject var theme: ThemeManager
    @EnvironmentObject var state: AppState
    var onClose: () -> Void = {}

    enum Tab: String, CaseIterable, Identifiable { case config, secrets, env
        var id: String { rawValue }
        var title: String {
            switch self {
            case .config: return "Config"
            case .secrets: return "Secrets"
            case .env: return "ENV inspector"
            }
        }
        var icon: String {
            switch self {
            case .config: return "chevron.left.forwardslash.chevron.right"
            case .secrets: return "key.fill"
            case .env: return "list.bullet.rectangle"
            }
        }
    }
    @State private var tab: Tab = .config

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "shippingbox.fill").font(.system(size: 13)).foregroundStyle(Theme.accent)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Session").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
                    Text(PomCore.shared.session.isEmpty ? "—" : PomCore.shared.session)
                        .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                }
                Spacer()
                Picker("", selection: $tab) {
                    ForEach(Tab.allCases) { t in Label(t.title, systemImage: t.icon).tag(t) }
                }.pickerStyle(.segmented).fixedSize()
                Button { onClose() } label: { Image(systemName: "xmark").font(.system(size: 12)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).padding(.leading, 6)
            }
            .padding(.horizontal, 18).padding(.vertical, 12)
            Divider().overlay(Theme.borderSoft)
            switch tab {
            case .config: AdvancedSettings()
            case .secrets: SecretsView()
            case .env: EnvInspector()
            }
        }
        .frame(width: 880, height: 600).background(Theme.bg)
        .sheet(isPresented: $state.showAgentSheet) {
            if let m = state.agentModel {
                AgentSheet(model: m, title: state.agentTitle, subtitle: state.agentSubtitle,
                           runningLabel: state.agentRunningLabel,
                           onBackground: { state.backgroundAgent() },
                           onDone: { state.endAgent() },
                           onStop: { state.endAgent() })
                    .environmentObject(state).environmentObject(theme)
            }
        }
    }
}
