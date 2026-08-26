import XCTest
@testable import PomeloApp

final class ServiceRunTests: XCTestCase {
    private func svc(_ name: String, running: Bool) -> Service {
        let json = "{\"name\":\"\(name)\",\"running\":\(running)}".data(using: .utf8)!
        return try! JSONDecoder().decode(Service.self, from: json)
    }

    func testStartTargetsStoppedOnly() {
        let out = ServiceRun.targets([svc("a", running: true), svc("b", running: false)], action: "start")
        XCTAssertEqual(out.map(\.name), ["b"])
    }

    func testStopTargetsRunningOnly() {
        let out = ServiceRun.targets([svc("a", running: true), svc("b", running: false)], action: "stop")
        XCTAssertEqual(out.map(\.name), ["a"])
    }

    func testRestartTargetsAll() {
        let out = ServiceRun.targets([svc("a", running: true), svc("b", running: false)], action: "restart")
        XCTAssertEqual(out.map(\.name), ["a", "b"])
    }

    func testUnknownActionTargetsNothing() {
        let out = ServiceRun.targets([svc("a", running: true)], action: "bogus")
        XCTAssertTrue(out.isEmpty)
    }
}
