import XCTest
@testable import PomeloRemoteKit

final class PairingTests: XCTestCase {
    func testParseQRPayload() {
        let qr = #"{"host":"192.168.1.5","port":"51234","token":"abc","fp":"deadbeef","scheme":"https"}"#
        let d = PairedDevice.fromQR(qr)
        XCTAssertNotNil(d)
        XCTAssertEqual(d?.host, "192.168.1.5")
        XCTAssertEqual(d?.port, "51234")
        XCTAssertEqual(d?.token, "abc")
        XCTAssertEqual(d?.fingerprint, "deadbeef")
        XCTAssertEqual(d?.baseURL?.absoluteString, "https://192.168.1.5:51234")
    }

    func testRejectsGarbageQR() {
        XCTAssertNil(PairedDevice.fromQR("not json"))
        XCTAssertNil(PairedDevice.fromQR(#"{"port":"1"}"#), "missing host must fail")
    }

    func testAgentFrameTakesFirstTextKey() {
        let f = PomJSON.decode(AgentFrame.self, from: Data(#"{"type":"text","v":"hello"}"#.utf8))
        XCTAssertEqual(f?.text, "hello")
        let f2 = PomJSON.decode(AgentFrame.self, from: Data(#"{"type":"assistant","text":"hi"}"#.utf8))
        XCTAssertEqual(f2?.text, "hi")
    }

    func testWorkspacesDecode() {
        let json = #"{"workspaces":[{"branch":"feat","is_main":false,"running":1,"total":3,"agents":[{"name":"claude","state":"working"}]}]}"#
        let p = PomJSON.decode(WorkspacesPayload.self, from: Data(json.utf8))
        XCTAssertEqual(p?.workspaces.count, 1)
        XCTAssertEqual(p?.workspaces.first?.running, 1)
        XCTAssertEqual(p?.workspaces.first?.agents.first?.state, "working")
    }
}
