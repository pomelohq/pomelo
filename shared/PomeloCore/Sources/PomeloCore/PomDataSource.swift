import Foundation

// The transport-neutral data seam. It mirrors the Go core's Dispatcher (query/command/
// fetch); the macOS app satisfies it in-process over the libpom FFI, the iOS app over
// the network (RemoteClient). Shared ViewModels depend on this, not on either transport,
// so they run unchanged on both platforms.
public protocol PomDataSource: Sendable {
    func query(_ domain: String, _ params: [String: Any]) async throws -> Data
    func command(_ domain: String, _ action: String, _ params: [String: Any]) async throws -> Data
    func fetch(_ domain: String, _ params: [String: Any]) async throws -> Data
}

public extension PomDataSource {
    func query(_ domain: String) async throws -> Data { try await query(domain, [:]) }
    func fetch(_ domain: String) async throws -> Data { try await fetch(domain, [:]) }
    func command(_ domain: String, _ action: String) async throws -> Data { try await command(domain, action, [:]) }

    // Query + decode in one step; returns nil on empty/failed decode (matches the apps'
    // existing tolerant decode behavior).
    func queryDecode<T: Decodable>(_ type: T.Type, _ domain: String, _ params: [String: Any] = [:]) async throws -> T? {
        PomJSON.decode(type, from: try await query(domain, params))
    }
    func fetchDecode<T: Decodable>(_ type: T.Type, _ domain: String, _ params: [String: Any] = [:]) async throws -> T? {
        PomJSON.decode(type, from: try await fetch(domain, params))
    }
}
