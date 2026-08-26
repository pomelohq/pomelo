import Foundation

// Which services a "start all" / "stop all" / "restart all" action targets,
// extracted from the view so it is unit-testable (MVVM): start touches only the
// stopped ones, stop only the running ones, restart everything.
enum ServiceRun {
    static func targets(_ services: [Service], action: String) -> [Service] {
        switch action {
        case "restart": return services
        case "start": return services.filter { !$0.running }
        case "stop": return services.filter { $0.running }
        default: return []
        }
    }
}
