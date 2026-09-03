import Foundation
import CPom
import AppKit
import SwiftTerm

enum StreamKind: Int32 {
    case json = 0, text = 1, binary = 2, close = 3
}

@MainActor
final class StreamManager {
    static let shared = StreamManager()
    private var clients: [Int32: (StreamKind, [UInt8]) -> Void] = [:]
    private var installed = false
    var activeStreamID: Int32 = 0
    private let terminals = NSMapTable<NSNumber, TerminalView>.strongToWeakObjects()
    private var claudeTermByWs: [String: Int32] = [:]
    private var pendingClaudeText: [String: String] = [:]

    func registerTerminal(_ id: Int32, view: TerminalView) {
        terminals.setObject(view, forKey: NSNumber(value: id))
    }

    func registerClaudeTerminal(_ id: Int32, wsKey: String) {
        guard id > 0 else { return }
        claudeTermByWs[wsKey] = id
        if let pending = pendingClaudeText[wsKey] {
            pendingClaudeText[wsKey] = nil
            // Let the freshly-attached claude prompt settle before typing into it.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in self?.sendText(id, pending) }
        }
    }

    func unregisterClaudeTerminal(_ id: Int32, wsKey: String) {
        if claudeTermByWs[wsKey] == id { claudeTermByWs[wsKey] = nil }
    }

    // Type text into the workspace's live Claude terminal (no newline — the reviewer
    // reads it and presses Enter). Queues until the terminal mounts if it isn't open.
    func askClaude(wsKey: String, text: String) {
        if let id = claudeTermByWs[wsKey], id > 0 { sendText(id, text) }
        else { pendingClaudeText[wsKey] = text }
    }

    func clearActive() {
        var target = activeStreamID
        if let fr = NSApp.keyWindow?.firstResponder as? NSView {
            let e = terminals.keyEnumerator()
            while let key = e.nextObject() as? NSNumber {
                if let v = terminals.object(forKey: key), v === fr || fr.isDescendant(of: v) {
                    target = key.int32Value; break
                }
            }
        }
        guard target > 0 else { return }
        send(target, [0x0c][...])
    }

    private init() {}

    func ensureCallback() {
        guard !installed else { return }
        installed = true
        PomSetStreamCallback(pomStreamTrampoline)
    }

    func openPTY(name: String, wsKey: String, cols: Int32, rows: Int32,
                 onFrame: @escaping (StreamKind, [UInt8]) -> Void) async -> Int32 {
        ensureCallback()
        let id = await Task.detached(priority: .userInitiated) {
            pomSubscribe("pty", ["name": name, "ws_key": wsKey, "cols": Int(cols), "rows": Int(rows)])
        }.value
        if id > 0 { clients[id] = onFrame; activeStreamID = id }
        return id
    }

    func openClaude(branch: String, isMain: Bool, mode: String, model: String, role: String,
                    onFrame: @escaping (StreamKind, [UInt8]) -> Void) -> Int32 {
        ensureCallback()
        let id = pomSubscribe("claude", ["branch": branch, "is_main": isMain, "mode": mode, "model": model, "role": role])
        if id > 0 { clients[id] = onFrame }
        return id
    }

    func openCreateWorkspace(branch: String, repos: [String], onFrame: @escaping (StreamKind, [UInt8]) -> Void) -> Int32 {
        ensureCallback()
        let id = pomSubscribe("create_workspace", ["branch": branch, "repos": repos.joined(separator: ",")])
        if id > 0 { clients[id] = onFrame }
        return id
    }

    func openAddRepo(branch: String, isMain: Bool, repos: [String], onFrame: @escaping (StreamKind, [UInt8]) -> Void) -> Int32 {
        ensureCallback()
        let id = pomSubscribe("add_repo", ["branch": branch, "is_main": isMain, "repos": repos.joined(separator: ",")])
        if id > 0 { clients[id] = onFrame }
        return id
    }

    func openDeleteWorkspace(branch: String, onFrame: @escaping (StreamKind, [UInt8]) -> Void) -> Int32 {
        ensureCallback()
        let id = pomSubscribe("delete_workspace", ["branch": branch])
        if id > 0 { clients[id] = onFrame }
        return id
    }

    func openPrepareMain(skipSeed: Bool, onFrame: @escaping (StreamKind, [UInt8]) -> Void) -> Int32 {
        ensureCallback()
        let id = pomSubscribe("prepare_main", ["skip_seed": skipSeed])
        if id > 0 { clients[id] = onFrame }
        return id
    }

    func sendText(_ id: Int32, _ text: String) {
        let bytes = Array(text.utf8)
        send(id, bytes[...])
    }

    func stopTurn(_ id: Int32) { PomStreamStop(id) }

    func send(_ id: Int32, _ bytes: ArraySlice<UInt8>) {
        var buf = Array(bytes)
        buf.withUnsafeMutableBytes { raw in
            PomStreamSend(id, raw.baseAddress?.assumingMemoryBound(to: CChar.self), Int32(raw.count))
        }
    }

    func resize(_ id: Int32, cols: Int32, rows: Int32) { PomStreamResize(id, cols, rows) }

    func close(_ id: Int32) {
        guard id > 0 else { return }
        PomStreamClose(id)
        clients[id] = nil
    }

    fileprivate func dispatch(id: Int32, kind: StreamKind, bytes: [UInt8]) {
        PerfHUD.shared.tick("pty:disp")
        clients[id]?(kind, bytes)
    }
}

private func pomSubscribe(_ topic: String, _ params: [String: Any]) -> Int32 {
    let data = (try? JSONSerialization.data(withJSONObject: params)) ?? Data("{}".utf8)
    let json = String(decoding: data, as: UTF8.self)
    return topic.withCString { t in json.withCString { p in
        PomSubscribe(UnsafeMutablePointer(mutating: t), UnsafeMutablePointer(mutating: p))
    }}
}

private func pomStreamTrampoline(_ id: Int32, _ kind: Int32, _ data: UnsafeMutableRawPointer?, _ len: Int32) {
    var bytes = [UInt8]()
    if let data = data, len > 0 {
        bytes = Array(UnsafeRawBufferPointer(start: data, count: Int(len)))
    }
    let k = StreamKind(rawValue: kind) ?? .binary
    DispatchQueue.main.async {
        StreamManager.shared.dispatch(id: id, kind: k, bytes: bytes)
    }
}
