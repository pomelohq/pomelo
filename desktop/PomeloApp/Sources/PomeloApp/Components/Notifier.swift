import Foundation
import AppKit
import UserNotifications

enum Notifier {
    private static var authorized = false
    private static let delegate = NotifierDelegate()

    static var onOpenWorkspace: ((String) -> Void)?

    static func requestAuth() {
        UNUserNotificationCenter.current().delegate = delegate
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { granted, _ in
            authorized = granted
        }
        UNUserNotificationCenter.current().getNotificationSettings { s in
            authorized = s.authorizationStatus == .authorized
        }
    }

    static func promptOrOpenSettings() {
        let c = UNUserNotificationCenter.current()
        c.delegate = delegate
        c.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            authorized = granted
            if granted { return }
            c.getNotificationSettings { s in
                if s.authorizationStatus != .authorized {
                    DispatchQueue.main.async { openNotificationSettings() }
                }
            }
        }
    }

    private static func openNotificationSettings() {
        for s in ["x-apple.systempreferences:com.apple.Notifications-Settings.extension",
                  "x-apple.systempreferences:com.apple.preference.notifications"] {
            if let url = URL(string: s), NSWorkspace.shared.open(url) { return }
        }
    }

    static func currentlyAuthorized(_ cb: @escaping (Bool) -> Void) {
        UNUserNotificationCenter.current().getNotificationSettings { s in
            let ok = s.authorizationStatus == .authorized
            authorized = ok
            DispatchQueue.main.async { cb(ok) }
        }
    }

    static func sendTest() {
        let c = UNUserNotificationCenter.current()
        c.delegate = delegate
        c.getNotificationSettings { s in
            guard s.authorizationStatus == .authorized else {
                DispatchQueue.main.async { promptOrOpenSettings() }
                return
            }
            authorized = true
            let content = UNMutableNotificationContent()
            content.title = "Pomelo"
            content.body = "Test notification — delivery is working."
            content.sound = .default
            c.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil), withCompletionHandler: nil)
        }
    }

    // Banner is silent; the per-event sound is played by SoundPrefs so it is
    // customizable and can fire even when the app is focused.
    static func notify(title: String, body: String, wsKey: String) {
        guard authorized else { return }
        let c = UNMutableNotificationContent()
        c.title = title
        c.body = body
        if !wsKey.isEmpty { c.userInfo = ["ws": wsKey] }
        let req = UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil)
        UNUserNotificationCenter.current().add(req, withCompletionHandler: nil)
    }

    static func message(ws: String, from: String?, to: String) -> (title: String, event: String)? {
        let wasWorking = from == "thinking" || from == "tool_use"
        let wasIdle = from == nil || from == "idle" || from == "stopped"
        if to == "awaiting_input" && from != "awaiting_input" {
            return ("Claude needs your input", "needs_input")
        }
        if to == "compacting" && from != "compacting" {
            return ("Claude is compacting", "compacting")
        }
        if wasWorking && (to == "idle" || to == "stopped") {
            return ("Claude finished", "finished")
        }
        if wasIdle && (to == "thinking" || to == "tool_use") {
            return ("Claude is working", "working")
        }
        return nil
    }
}

private final class NotifierDelegate: NSObject, UNUserNotificationCenterDelegate {
    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        let ws = response.notification.request.content.userInfo["ws"] as? String
        DispatchQueue.main.async {
            NSApp.activate(ignoringOtherApps: true)
            NSApp.windows.first?.makeKeyAndOrderFront(nil)
            if let ws, !ws.isEmpty { Notifier.onOpenWorkspace?(ws) }
        }
        completionHandler()
    }
}
