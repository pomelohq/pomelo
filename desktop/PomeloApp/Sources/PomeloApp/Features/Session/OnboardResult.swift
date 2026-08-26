import SwiftUI

// Groups onboarding's unresolved findings by what the user must do, so the
// failure state leads with actionable items (credentials) instead of a flat
// "onboarding failed". Mirrors the Go summarizeOnboard categories. Pure +
// testable; the view below renders it.
enum OnboardCat: String, CaseIterable {
    case secret, boot, setup, config

    var headline: String {
        switch self {
        case .secret: return "Needs a credential from you"
        case .boot: return "Service didn't boot"
        case .setup: return "Dependency install failed"
        case .config: return "Config gap"
        }
    }

    var symbol: String {
        switch self {
        case .secret: return "key.fill"
        case .boot: return "bolt.slash"
        case .setup: return "shippingbox"
        case .config: return "exclamationmark.triangle"
        }
    }

    static func of(_ id: String) -> OnboardCat {
        if id.hasPrefix("secret.") { return .secret }
        if id.hasPrefix("boot.") { return .boot }
        if id.hasPrefix("setup.") { return .setup }
        return .config
    }
}

struct OnboardSection: Identifiable {
    let cat: OnboardCat
    let findings: [DoctorViewModel.Finding]
    var id: String { cat.rawValue }
}

enum OnboardResult {
    // Non-empty groups in the order the user should act on them.
    static func sections(_ findings: [DoctorViewModel.Finding]) -> [OnboardSection] {
        OnboardCat.allCases.compactMap { cat in
            let f = findings.filter { OnboardCat.of($0.id) == cat }
            return f.isEmpty ? nil : OnboardSection(cat: cat, findings: f)
        }
    }
}

struct OnboardResultView: View {
    let findings: [DoctorViewModel.Finding]
    let services: Int
    var onRetry: () -> Void = {}

    private var sections: [OnboardSection] { OnboardResult.sections(findings) }
    private var bootFails: Int { findings.filter { OnboardCat.of($0.id) == .boot }.count }
    private var headline: String {
        let items = "\(findings.count) item(s) need attention."
        return services > 0
            ? "Config authored. Verified \(max(0, services - bootFails))/\(services) services. \(items)"
            : "Config authored. \(items)"
    }

    var body: some View {
        if findings.isEmpty {
            HStack(spacing: 8) {
                Image(systemName: "checkmark.seal.fill").foregroundStyle(Theme.ok)
                Text("Config authored — project is runnable, no blocking gaps.")
                    .font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.fg)
                Spacer()
            }.padding(16)
        } else {
            content
        }
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(headline)
                .font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.fg)
            ForEach(sections) { section in
                VStack(alignment: .leading, spacing: 6) {
                    Label(section.cat.headline, systemImage: section.cat.symbol)
                        .font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.accent)
                    ForEach(section.findings) { f in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(f.title).font(.system(size: 12)).foregroundStyle(Theme.fg)
                            if !f.detail.isEmpty {
                                Text(f.detail).font(.system(size: 11)).foregroundStyle(Theme.fgMuted).lineLimit(3)
                            }
                            if !f.fix.isEmpty {
                                Text(f.fix).font(.system(size: 11)).foregroundStyle(Theme.dim)
                            }
                        }
                        .padding(.leading, 20)
                    }
                }
            }
            HStack {
                Spacer()
                Button("Retry", action: onRetry).buttonStyle(.borderedProminent).tint(Theme.accent)
            }
        }
        .padding(16)
    }
}
