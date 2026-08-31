import SwiftUI
import CoreImage.CIFilterBuiltins
import Foundation

struct RemoteInfo: Decodable {
    var enabled = false
    var host = ""
    var port = ""
    var token = ""
    var fingerprint = ""
    var hosts: [String] = []
    init() {}
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? false
        host = try c.decodeIfPresent(String.self, forKey: .host) ?? ""
        port = try c.decodeIfPresent(String.self, forKey: .port) ?? ""
        token = try c.decodeIfPresent(String.self, forKey: .token) ?? ""
        fingerprint = try c.decodeIfPresent(String.self, forKey: .fingerprint) ?? ""
        hosts = try c.decodeIfPresent([String].self, forKey: .hosts) ?? []
    }
    enum K: String, CodingKey { case enabled, host, port, token, fingerprint, hosts }

    // The exact payload the phone scans/pastes: everything it needs to dial + pin.
    // Base64 so it's one opaque token to copy-paste (no stray quotes/braces).
    // `hosts` carries every reachable IP (LAN + Tailscale) so the phone can pick.
    var pairingJSON: String {
        var obj: [String: Any] = ["host": host, "port": port, "token": token, "fp": fingerprint, "scheme": "https"]
        if !hosts.isEmpty { obj["hosts"] = hosts }
        guard let d = try? JSONSerialization.data(withJSONObject: obj) else { return "" }
        return d.base64EncodedString()
    }
    var reachable: Bool { !host.isEmpty && !port.isEmpty }
}

@MainActor
final class RemoteBonjour: ObservableObject {
    private var service: NetService?
    func advertise(port: String) {
        stop()
        guard let p = Int32(port), p > 0 else { return }
        let s = NetService(domain: "", type: "_pomelo._tcp.", name: Host.current().localizedName ?? "Pomelo", port: p)
        s.publish()
        service = s
    }
    func stop() { service?.stop(); service = nil }
}

struct RemoteControlSettings: View {
    @State private var info = RemoteInfo()
    @State private var busy = false
    @StateObject private var bonjour = RemoteBonjour()

    var body: some View {
        Section {
            Toggle("Enable remote control", isOn: Binding(
                get: { info.enabled },
                set: { on in Task { await toggle(on) } }
            )).disabled(busy)

            if info.enabled, info.reachable {
                LabeledContent("Address") {
                    Text("\(info.host):\(info.port)").monospaced().textSelection(.enabled)
                }
                LabeledContent("Fingerprint") {
                    Text(shortFP).monospaced().font(.system(size: 10)).foregroundStyle(Theme.fgMuted).textSelection(.enabled)
                }
                HStack {
                    Spacer()
                    if let img = qr(info.pairingJSON) {
                        VStack(spacing: 6) {
                            Image(nsImage: img).interpolation(.none).resizable()
                                .frame(width: 160, height: 160)
                                .background(.white).clipShape(RoundedRectangle(cornerRadius: 8))
                            Text("Scan in the Pomelo iPhone app").font(.system(size: 10)).foregroundStyle(Theme.fgMuted)
                        }
                    }
                    Spacer()
                }
                .padding(.vertical, 4)
            }
        } header: { Text("Remote control (LAN)") } footer: {
            Text("Serves a TLS control API on your local network so the Pomelo iPhone app can monitor this Mac. Pair by scanning the QR. To reach it away from the LAN, put both devices on the same VPN. Read-only plus agent nudges; it cannot start services or change git.")
        }
        .task { await load() }
        .onDisappear { bonjour.stop() }
    }

    private var shortFP: String {
        guard info.fingerprint.count >= 16 else { return info.fingerprint }
        let s = info.fingerprint
        return s.prefix(8) + "…" + s.suffix(8)
    }

    private func load() async {
        if let r = PomJSON.decode(RemoteInfo.self, from: await SettingsStore.remoteInfo()) {
            info = r
            if r.enabled, r.reachable { bonjour.advertise(port: r.port) }
        }
    }

    private func toggle(_ on: Bool) async {
        busy = true
        _ = await SettingsStore.remoteSet(enabled: on)
        await load()
        if !on { bonjour.stop() }
        busy = false
    }

    private func qr(_ text: String) -> NSImage? {
        guard !text.isEmpty else { return nil }
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let out = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 8, y: 8)) else { return nil }
        let ctx = CIContext()
        guard let cg = ctx.createCGImage(out, from: out.extent) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: out.extent.width, height: out.extent.height))
    }
}
