import Foundation
// PomDataSource + PomJSON live in the shared PomeloCore package; re-export so existing
// PomJSON.* call sites need no per-file import.
@_exported import PomeloCore

// PomCore satisfies the shared async data seam by running the blocking libpom FFI on a
// background queue (never the main thread). Existing synchronous typed methods are kept;
// this is additive so shared ViewModels can depend on PomDataSource.
extension PomCore: PomDataSource {
    private static let ffiQueue = DispatchQueue(label: "pom.ffi.async", attributes: .concurrent)

    func query(_ domain: String, _ params: [String: Any]) async throws -> Data {
        let data = Self.jsonParams(params)
        return await withCheckedContinuation { c in
            Self.ffiQueue.async { c.resume(returning: self.query(domain: domain, params: data)) }
        }
    }
    func command(_ domain: String, _ action: String, _ params: [String: Any]) async throws -> Data {
        let data = Self.jsonParams(params)
        return await withCheckedContinuation { c in
            Self.ffiQueue.async { c.resume(returning: self.command(domain: domain, action: action, params: data)) }
        }
    }
    func fetch(_ domain: String, _ params: [String: Any]) async throws -> Data {
        let data = Self.jsonParams(params)
        return await withCheckedContinuation { c in
            Self.ffiQueue.async { c.resume(returning: self.fetch(domain: domain, params: data)) }
        }
    }

    private static func jsonParams(_ d: [String: Any]) -> Data {
        d.isEmpty ? Data("{}".utf8) : ((try? JSONSerialization.data(withJSONObject: d)) ?? Data("{}".utf8))
    }
}
