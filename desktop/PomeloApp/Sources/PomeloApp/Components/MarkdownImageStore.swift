import AppKit

actor MarkdownImageCache {
    static let shared = MarkdownImageCache()
    private var cache: [String: NSImage] = [:]

    func image(for url: String) async -> NSImage? {
        if let c = cache[url] { return c }
        let data = await Task.detached(priority: .utility) { PomCore.shared.fetchImageData(url: url) }.value
        struct R: Decodable { var ok = false; var b64 = "" }
        guard let r = PomJSON.decode(R.self, from: data), r.ok,
              let raw = Data(base64Encoded: r.b64), let img = NSImage(data: raw) else { return nil }
        cache[url] = img
        return img
    }
}
