import Foundation
import Sparkle

@MainActor
final class AppUpdater: ObservableObject {
    static let shared = AppUpdater()

    private let controller: SPUStandardUpdaterController?

    private init() {
        // Sparkle requires a real .app bundle; a bare dev binary (build.sh run)
        // has none and would crash it. Skip the updater when running unbundled.
        if Bundle.main.bundleURL.pathExtension == "app" {
            controller = SPUStandardUpdaterController(startingUpdater: true,
                                                      updaterDelegate: nil,
                                                      userDriverDelegate: nil)
        } else {
            controller = nil
        }
    }

    func checkForUpdates() { controller?.updater.checkForUpdates() }

    var canCheckForUpdates: Bool { controller?.updater.canCheckForUpdates ?? false }

    var automaticChecks: Bool {
        get { controller?.updater.automaticallyChecksForUpdates ?? false }
        set { controller?.updater.automaticallyChecksForUpdates = newValue }
    }
}
