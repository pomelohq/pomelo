import XCTest
@testable import PomeloApp

// Guards the Core<->UI contract (ADR 0001): every DTO must decode the minimal JSON
// the Go core actually emits. Go omits `omitempty` keys on success and marshals
// slices as `[]`, so a UI model that requires an absent key, or chokes on an
// omitted-optional, is a drift bug. Each case here is a shape Go really produces.
final class ContractTests: XCTestCase {
    private func decode<T: Decodable>(_ type: T.Type, _ json: String,
                                      file: StaticString = #filePath, line: UInt = #line) -> T? {
        guard let data = json.data(using: .utf8) else { return nil }
        do { return try JSONDecoder().decode(type, from: data) }
        catch { XCTFail("\(type) failed to decode \(json): \(error)", file: file, line: line); return nil }
    }

    // The usage chip bug: on success Go drops `error` (omitempty); a bare failure
    // has only `error`; a missing token yields `{}`. All three must decode.
    func testClaudeUsageToleratesOmittedKeys() {
        XCTAssertEqual(decode(ClaudeUsage.self,
            #"{"ok":true,"session":{"pct":18,"resets_at":123},"weekly":{"pct":77,"resets_at":456}}"#)?.session?.pct, 18)
        XCTAssertEqual(decode(ClaudeUsage.self, #"{"ok":false,"error":"no creds"}"#)?.error, "no creds")
        XCTAssertNotNil(decode(ClaudeUsage.self, "{}"))
    }

    // ws_services / display_name are omitempty; repos must arrive as [] not null.
    func testWorkspaceOmitsOptionalServices() {
        let ws = decode(Workspace.self,
            #"{"branch":"main","is_main":true,"path":"/p","repos":[],"running":0,"total":0}"#)
        XCTAssertEqual(ws?.branch, "main")
        XCTAssertNil(ws?.wsServices)
        XCTAssertEqual(ws?.repos.count, 0)
    }

    func testWorkspacesResponseEmpty() {
        XCTAssertEqual(decode(WorkspacesResponse.self, #"{"workspaces":[]}"#)?.workspaces.count, 0)
    }

    // NMStore: consumers must be [] (nil slice -> null would throw); all envelope
    // keys are always present.
    func testNMStorePayload() {
        let p = decode(NMStoreViewModel.Payload.self,
            #"{"entries":[{"repo":"web","hash":"abc","bytes":10,"current":false,"orphan":true,"consumers":[]}],"total":10,"unoptimized":[]}"#)
        XCTAssertEqual(p?.entries.first?.repo, "web")
        XCTAssertTrue(p?.entries.first?.orphan == true)
        XCTAssertEqual(p?.total, 10)
    }

    // PR status tokens are decided by the core; the UI decodes them directly. Go
    // always emits checks/review/conflict/reviewers (no omitempty) — the synthesized
    // Decodable throws on an absent non-optional key, so the contract is "always present".
    func testPRInfoDecodesCoreTokens() {
        let json = #"{"number":7,"title":"t","state":"OPEN","url":"u","isDraft":false,"checks":"fail","review":"changes","conflict":true,"reviewers":[{"name":"bob","state":"approved"}],"statusCheckRollup":[{"name":"build","result":"fail"}]}"#
        let pr = decode(PRInfo.self, json)
        XCTAssertEqual(pr?.checks, .fail)
        XCTAssertEqual(pr?.review, .changes)
        XCTAssertEqual(pr?.conflict, true)
        XCTAssertEqual(pr?.reviewers.first?.state, "approved")
        XCTAssertEqual(pr?.statusCheckRollup?.first?.result, .fail)
    }

    // The clean case: none/none/false and an empty reviewers array still decode.
    func testPRInfoCleanTokens() {
        let json = #"{"number":1,"title":"t","state":"OPEN","url":"u","isDraft":false,"checks":"none","review":"none","conflict":false,"reviewers":[]}"#
        let pr = decode(PRInfo.self, json)
        XCTAssertEqual(pr?.checks, PRInfo.ChecksStatus.none)
        XCTAssertEqual(pr?.conflict, false)
        XCTAssertEqual(pr?.reviewers.count, 0)
    }

    func testAllPRsGroupEnvelope() {
        let g = decode(PRsViewModel.Group.self,
            #"{"prs":[{"repo":"api","alias":"api","behind":0,"ahead":0,"pr":null}],"severity":"danger"}"#)
        XCTAssertEqual(g?.severity, "danger")
        XCTAssertEqual(g?.prs.first?.repo, "api")
    }

    func testSessionsResponseOptionalCurrent() {
        let r = decode(SessionsResponse.self,
            #"{"sessions":[{"name":"demo","current":true,"running":true}]}"#)
        XCTAssertEqual(r?.sessions.first?.name, "demo")
        XCTAssertNil(r?.current)
        XCTAssertTrue(r?.sessions.first?.isAvailable == true)
    }
}
