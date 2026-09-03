import SwiftUI

struct Avatar: View {
    let url: String?
    let name: String?
    var size: CGFloat = 22
    // Hosts that need credentials (the Jira site) are fetched through the core.
    var viaCore = false
    @State private var coreImage: NSImage?

    // GitHub serves avatars at the requested size; ask for 2x so they stay crisp on Retina.
    private var sized: URL? {
        guard let url, var c = URLComponents(string: url) else { return nil }
        c.queryItems = (c.queryItems ?? []).filter { $0.name != "s" } + [URLQueryItem(name: "s", value: String(Int(size * 2)))]
        return c.url
    }

    // The URL actually fetched: Jira as-is; others (GitHub) size-tagged. Both resolve
    // through MarkdownImageCache so an avatar is fetched+decoded once and reused.
    // SwiftUI's AsyncImage caches neither, so a conversation scroll used to re-fetch
    // and re-decode every avatar on each pass — the scroll freeze.
    private var fetchURL: String? { viaCore ? url : sized?.absoluteString }

    var body: some View {
        Group {
            if let img = coreImage { Image(nsImage: img).resizable().scaledToFill() } else { fallback }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(Theme.borderSoft))
        .task(id: fetchURL) {
            guard let u = fetchURL, !u.isEmpty else { return }
            coreImage = await MarkdownImageCache.shared.image(for: u)
        }
    }

    private var fallback: some View {
        ZStack {
            Theme.panel3
            Text(String((name ?? "?").prefix(1)).uppercased())
                .font(.system(size: size * 0.45, weight: .semibold)).foregroundStyle(Theme.fgMuted)
        }
    }
}
