import SwiftUI

// Shared building blocks for the three states every async panel needs: loading,
// empty, and a status chip. Keeps the look consistent and kills the copy-pasted
// `centered("loading…")` / ad-hoc "No X" / mini-capsule variants scattered around.

struct LoadingView: View {
    var text: String? = nil
    var body: some View {
        VStack(spacing: 10) {
            Spinner(size: 16)
            if let t = text {
                Text(t).font(.system(size: 12)).foregroundStyle(Theme.dim)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct EmptyStateView: View {
    var icon: String
    var title: String
    var subtitle: String? = nil
    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: icon).font(.system(size: 26)).foregroundStyle(Theme.dim)
            Text(title).font(.system(size: 12.5)).foregroundStyle(Theme.fgMuted)
            if let s = subtitle {
                Text(s).font(.system(size: 11)).foregroundStyle(Theme.dim)
                    .multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// The ubiquitous tinted capsule chip (CI, approved, conflict, jira status, …).
struct Badge: View {
    var icon: String? = nil
    var text: String
    var color: Color
    var body: some View {
        HStack(spacing: 3) {
            if let i = icon { Image(systemName: i).font(.system(size: 8.5)) }
            Text(text).font(.system(size: 9.5, weight: .medium))
        }
        .foregroundStyle(color)
        .padding(.horizontal, 5).padding(.vertical, 1)
        .background(color.opacity(0.13), in: Capsule())
    }
}
