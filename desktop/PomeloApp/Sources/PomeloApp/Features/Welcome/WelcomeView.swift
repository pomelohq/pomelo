import SwiftUI
import AppKit

struct WelcomeView: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
            Image(nsImage: NSApp.applicationIconImage)
                .resizable().frame(width: 92, height: 92)
                .clipShape(RoundedRectangle(cornerRadius: 20))
            Text("Welcome to Pomelo").font(.system(size: 22, weight: .bold)).foregroundStyle(Theme.fg).padding(.top, 18)
            Text("A dev environment per branch. Open a session to get started,\nor start a new one — Pomelo scaffolds the rest.")
                .font(.system(size: 13)).foregroundStyle(Theme.fgMuted)
                .multilineTextAlignment(.center).lineSpacing(3).padding(.top, 8)

            HStack(spacing: 12) {
                Button { state.openExistingSession() } label: {
                    label("folder", "Open a session…", "a folder with pom.yml")
                }.buttonStyle(.plain)
                Button { state.showCreateSession = true } label: {
                    label("plus.rectangle.on.folder", "New session…", "add repos, agent writes the config")
                }.buttonStyle(.plain)
            }
            .padding(.top, 26)

            if let err = state.bootError {
                Text(err).font(.system(size: 12)).foregroundStyle(Theme.danger)
                    .multilineTextAlignment(.center).padding(.top, 18).frame(maxWidth: 420)
            }
            Spacer()
            Text("macOS 14+ · Notarized · No account")
                .font(Theme.mono(11)).foregroundStyle(Theme.dim).padding(.bottom, 22)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.bg)
    }

    private func label(_ icon: String, _ title: String, _ sub: String) -> some View {
        VStack(spacing: 8) {
            Image(systemName: icon).font(.system(size: 22)).foregroundStyle(Theme.accent)
            Text(title).font(.system(size: 13.5, weight: .semibold)).foregroundStyle(Theme.fg)
            Text(sub).font(.system(size: 11)).foregroundStyle(Theme.dim)
        }
        .frame(width: 210, height: 132)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Theme.borderSoft))
    }
}
