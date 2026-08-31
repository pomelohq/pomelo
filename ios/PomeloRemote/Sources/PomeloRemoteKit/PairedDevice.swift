import Foundation

struct PairedDevice: Codable, Identifiable, Hashable {
    var host: String
    var port: String
    var token: String
    var fingerprint: String
    var scheme: String = "https"
    var name: String = ""
    var id: String { "\(host):\(port)" }

    var baseURL: URL? { URL(string: "\(scheme)://\(host):\(port)") }

    static func fromQR(_ text: String) -> PairedDevice? { parse(text)?.device }

    static func parse(_ text: String) -> (device: PairedDevice, hosts: [String])? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let jsonData = Data(base64Encoded: trimmed) ?? trimmed.data(using: .utf8)
        guard let data = jsonData,
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let host = obj["host"] as? String, let port = obj["port"] as? String,
              let token = obj["token"] as? String, let fp = obj["fp"] as? String, !host.isEmpty
        else { return nil }
        let scheme = (obj["scheme"] as? String) ?? "https"
        var hosts = (obj["hosts"] as? [String]) ?? []
        if hosts.isEmpty { hosts = [host] }
        if !hosts.contains(host) { hosts.insert(host, at: 0) }
        let dev = PairedDevice(host: host, port: port, token: token, fingerprint: fp, scheme: scheme)
        return (dev, hosts)
    }
}

public final class DeviceStore: ObservableObject {
    @Published private(set) var devices: [PairedDevice] = []
    private let key = "pomelo.paired.devices"

    public init() { load() }

    func load() {
        guard let data = UserDefaults.standard.data(forKey: key),
              let list = try? JSONDecoder().decode([PairedDevice].self, from: data) else { return }
        devices = list
    }

    func add(_ d: PairedDevice) {
        devices.removeAll { $0.id == d.id }
        devices.append(d)
        save()
    }

    func remove(_ d: PairedDevice) {
        devices.removeAll { $0.id == d.id }
        save()
    }

    func rename(_ d: PairedDevice, to name: String) {
        guard let i = devices.firstIndex(where: { $0.id == d.id }) else { return }
        devices[i].name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        save()
    }

    func setAddress(_ d: PairedDevice, host: String, port: String) {
        guard let i = devices.firstIndex(where: { $0.id == d.id }) else { return }
        let h = host.trimmingCharacters(in: .whitespacesAndNewlines)
        let p = port.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !h.isEmpty else { return }
        devices[i].host = h
        if !p.isEmpty { devices[i].port = p }
        save()
    }

    private func save() {
        if let data = try? JSONEncoder().encode(devices) {
            UserDefaults.standard.set(data, forKey: key)
        }
    }
}
