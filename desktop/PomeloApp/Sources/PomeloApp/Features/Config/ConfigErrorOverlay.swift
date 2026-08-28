import SwiftUI

struct ConfigErrorOverlay: View {
    let message: String
    let onOpenConfig: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 30)).foregroundStyle(Theme.warn)
            Text("This session's config has errors")
                .font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            Text("Pomelo is running — fix the config to use this session, or switch to another from the header.")
                .font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                .multilineTextAlignment(.center).frame(maxWidth: 420)
            Card(background: Theme.bg) {
                ScrollView {
                    Text(message)
                        .font(Theme.mono(11)).foregroundStyle(Theme.danger)
                        .frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled).padding(10)
                }
                .frame(maxWidth: 560, maxHeight: 240)
            }
            Button { onOpenConfig() } label: {
                HStack(spacing: 6) { Image(systemName: "chevron.left.forwardslash.chevron.right"); Text("Open config") }
                    .font(.system(size: 12.5, weight: .medium))
            }.buttonStyle(.borderedProminent).tint(Theme.accent)
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.bg)
    }
}
