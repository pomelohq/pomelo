import SwiftUI

@main
struct PomeloRemoteApp: App {
    @StateObject private var store = DeviceStore()
    @StateObject private var theme = ThemeManager()
    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(store)
                .environmentObject(theme)
                .preferredColorScheme(theme.mode == .light ? .light : .dark)
        }
    }
}
