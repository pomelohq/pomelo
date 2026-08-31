import Foundation
#if canImport(ActivityKit)
import ActivityKit
#endif
import WidgetKit

@MainActor
final class LiveActivityController {
    static let shared = LiveActivityController()
    private init() {}

    #if canImport(ActivityKit)
    @available(iOS 16.1, *)
    private var activity: Activity<PomeloAgentAttributes>? {
        get { _activity as? Activity<PomeloAgentAttributes> }
        set { _activity = newValue }
    }
    private var _activity: Any?
    #endif

    private var lastSignature = ""

    func publish(mac: String, snapshot: AgentSnapshot, sessionPct: Int = 0, weeklyPct: Int = 0) {
        let sig = signature(snapshot) + "|u:\(sessionPct)/\(weeklyPct)"
        SharedStore.save(snapshot)
        guard sig != lastSignature else { return }
        lastSignature = sig
        WidgetCenter.shared.reloadAllTimelines()
        syncLiveActivity(mac: mac, snapshot: snapshot, sessionPct: sessionPct, weeklyPct: weeklyPct)
    }

    private func signature(_ s: AgentSnapshot) -> String {
        s.workspaces
            .sorted { $0.id < $1.id }
            .map { "\($0.id):\($0.state):\($0.running)/\($0.total):\($0.prCount)" }
            .joined(separator: "|")
    }

    private func syncLiveActivity(mac: String, snapshot: AgentSnapshot, sessionPct: Int, weeklyPct: Int) {
        #if canImport(ActivityKit)
        guard #available(iOS 16.1, *) else { return }
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }

        let active = snapshot.workspaces
            .filter { agentStateActive($0.state) }
            .sorted { ($0.state == "awaiting_input" ? 0 : 1) < ($1.state == "awaiting_input" ? 0 : 1) }

        guard let top = active.first else {
            endActivity()
            return
        }
        let content = PomeloAgentAttributes.ContentState(
            activeCount: active.count, headline: top.title, state: top.state,
            sessionPct: sessionPct, weeklyPct: weeklyPct)

        if activity == nil { activity = Activity<PomeloAgentAttributes>.activities.first }
        if let act = activity {
            Task { await act.update(using: content) }
        } else {
            do {
                activity = try Activity.request(
                    attributes: PomeloAgentAttributes(mac: mac),
                    contentState: content, pushType: nil)
            } catch {
            }
        }
        #endif
    }

    func endActivity() {
        #if canImport(ActivityKit)
        guard #available(iOS 16.1, *), let act = activity else { return }
        activity = nil
        Task { await act.end(dismissalPolicy: .immediate) }
        #endif
    }
}
