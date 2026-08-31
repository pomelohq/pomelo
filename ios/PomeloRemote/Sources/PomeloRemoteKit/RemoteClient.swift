import Foundation
import CryptoKit

enum RemoteError: Error { case badURL, http(Int), notPaired }

final class RemoteClient: NSObject, URLSessionDelegate {
    let device: PairedDevice
    private lazy var session: URLSession = {
        let cfg = URLSessionConfiguration.ephemeral
        cfg.timeoutIntervalForRequest = 20
        cfg.waitsForConnectivity = true
        return URLSession(configuration: cfg, delegate: self, delegateQueue: nil)
    }()

    init(device: PairedDevice) { self.device = device }

    func query(_ domain: String, _ params: [String: Any] = [:]) async throws -> Data {
        try await rpc("/rpc/query", ["domain": domain, "params": params])
    }

    func command(_ domain: String, _ action: String, _ params: [String: Any] = [:]) async throws -> Data {
        try await rpc("/rpc/command", ["domain": domain, "action": action, "params": params])
    }

    func fetch(_ domain: String, _ params: [String: Any] = [:]) async throws -> Data {
        try await rpc("/rpc/fetch", ["domain": domain, "params": params])
    }

    func nudge(branch: String, isMain: Bool, text: String) async throws {
        _ = try await rpc("/rpc/agent/send", ["branch": branch, "is_main": isMain, "text": text])
    }

    func claudeTerminalWindow(branch: String, isMain: Bool) async throws -> String {
        let data = try await query("claude_terminal", ["branch": branch, "is_main": isMain])
        struct T: Decodable { var window = ""; init(from d: Decoder) throws {
            window = try d.container(keyedBy: K.self).decodeIfPresent(String.self, forKey: .window) ?? "" }
            enum K: String, CodingKey { case window } }
        return (PomJSON.decode(T.self, from: data)?.window) ?? ""
    }

    func ptyInput(window: String, data: [UInt8]) async {
        _ = try? await rpc("/rpc/pty/input", ["window": window, "data_b64": Data(data).base64EncodedString()])
    }

    func ptyResize(window: String, cols: Int, rows: Int) async {
        _ = try? await rpc("/rpc/pty/resize", ["window": window, "cols": cols, "rows": rows])
    }

    enum PTYFrame { case output([UInt8]); case control(String) }

    func ptyStream(window: String, cols: Int, rows: Int) -> AsyncThrowingStream<PTYFrame, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var comps = URLComponents(url: device.baseURL!.appendingPathComponent("/rpc/stream/pty"),
                                              resolvingAgainstBaseURL: false)!
                    comps.queryItems = [
                        .init(name: "token", value: device.token),
                        .init(name: "window", value: window),
                        .init(name: "cols", value: String(cols)),
                        .init(name: "rows", value: String(rows)),
                    ]
                    guard let url = comps.url else { throw RemoteError.badURL }
                    let (bytes, resp) = try await session.bytes(for: URLRequest(url: url))
                    if let code = (resp as? HTTPURLResponse)?.statusCode, code != 200 {
                        throw RemoteError.http(code)
                    }
                    for try await line in bytes.lines {
                        if Task.isCancelled { break }
                        guard line.hasPrefix("data:") else { continue }
                        let payload = String(line.dropFirst(5).drop(while: { $0 == " " }))
                        guard let tag = payload.first else { continue }
                        let rest = String(payload.dropFirst())
                        if tag == "b", let d = Data(base64Encoded: rest) {
                            continuation.yield(.output([UInt8](d)))
                        } else if tag == "c" {
                            continuation.yield(.control(rest))
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    func jiraIssues(branches: [String]) async throws -> Data {
        try await query("jira_issues", ["branches": branches])
    }

    func prAll() async throws -> Data { try await fetch("pr_all") }

    func claudeUsage() async throws -> Data { try await query("claude_usage") }

    func wsOrder() async throws -> Data { try await query("ws_order") }

    func jiraIssue(key: String, force: Bool = false) async throws -> Data {
        try await query("jira_issue", ["key": key, "force": force])
    }

    func prWorkspace(branch: String, isMain: Bool) async throws -> Data {
        try await fetch("pr_workspace", ["branch": branch, "is_main": isMain])
    }

    func prRefresh() async { _ = try? await command("pr", "refresh") }

    func closeAgent(window: String) async { _ = try? await command("agent", "close", ["window": window]) }

    func createWorkspace(branch: String, repos: [String], displayName: String) async throws -> Data {
        try await command("workspace", "create", ["branch": branch, "repos": repos, "display_name": displayName])
    }

    func renameWorkspace(branch: String, isMain: Bool, displayName: String) async throws -> Data {
        try await command("workspace", "rename", ["branch": branch, "is_main": isMain, "display_name": displayName])
    }

    func repos() async throws -> Data { try await query("repos") }

    func jiraBoards() async throws -> Data { try await query("jira_boards") }
    func jiraSprint(board: Int) async throws -> Data { try await query("jira_sprint", ["board": board]) }

    func suggestName(branch: String, desc: String) async throws -> Data {
        try await query("suggest_name", ["branch": branch, "desc": desc])
    }

    func prDetail(branch: String, repo: String, isMain: Bool) async throws -> Data {
        try await fetch("pr_detail", ["branch": branch, "repo": repo, "is_main": isMain])
    }

    func prDiff(branch: String, repo: String, isMain: Bool) async throws -> Data {
        try await fetch("pr_diff", ["branch": branch, "repo": repo, "is_main": isMain])
    }

    func prTimeline(branch: String, repo: String, isMain: Bool) async throws -> Data {
        try await fetch("pr_timeline", ["branch": branch, "repo": repo, "is_main": isMain])
    }

    func ping() async throws -> Bool {
        guard let url = device.baseURL?.appendingPathComponent("ping") else { throw RemoteError.badURL }
        let (_, resp) = try await session.data(from: url)
        return (resp as? HTTPURLResponse)?.statusCode == 200
    }

    private func rpc(_ path: String, _ body: [String: Any]) async throws -> Data {
        guard let url = device.baseURL?.appendingPathComponent(path) else { throw RemoteError.badURL }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("Bearer \(device.token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await session.data(for: req)
        if let code = (resp as? HTTPURLResponse)?.statusCode, code != 200 {
            throw RemoteError.http(code)
        }
        return data
    }

    func agentStream(branch: String, isMain: Bool, mode: String = "") -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var comps = URLComponents(url: device.baseURL!.appendingPathComponent("/rpc/stream/agent"),
                                              resolvingAgainstBaseURL: false)!
                    comps.queryItems = [
                        .init(name: "token", value: device.token),
                        .init(name: "branch", value: branch),
                        .init(name: "is_main", value: isMain ? "1" : "0"),
                        .init(name: "mode", value: mode),
                    ]
                    guard let url = comps.url else { throw RemoteError.badURL }
                    let (bytes, resp) = try await session.bytes(for: URLRequest(url: url))
                    if let code = (resp as? HTTPURLResponse)?.statusCode, code != 200 {
                        throw RemoteError.http(code)
                    }
                    for try await line in bytes.lines {
                        if Task.isCancelled { break }
                        if line.hasPrefix("data:") {
                            let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                            if let d = payload.data(using: .utf8) { continuation.yield(d) }
                        }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust,
              let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
              let leaf = chain.first
        else {
            completionHandler(.cancelAuthenticationChallenge, nil)
            return
        }
        let der = SecCertificateCopyData(leaf) as Data
        let fp = SHA256.hash(data: der).map { String(format: "%02x", $0) }.joined()
        if fp == device.fingerprint {
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }
}
