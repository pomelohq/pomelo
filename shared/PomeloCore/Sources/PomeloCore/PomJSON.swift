import Foundation
import OSLog

public enum PomJSON {
    public static let decoder = JSONDecoder()
    private static let log = Logger(subsystem: "com.pomelo.app", category: "decode")

    public static func decode<T: Decodable>(_ type: T.Type, from data: Data) -> T? {
        if data.isEmpty { return nil }
        do {
            return try decoder.decode(type, from: data)
        } catch {
            log.debug("decode \(String(describing: type)) failed: \(error.localizedDescription) — \(String(decoding: data.prefix(240), as: UTF8.self), privacy: .public)")
            return nil
        }
    }
}
