import XCTest
@testable import PomeloApp

final class OnboardResultTests: XCTestCase {
    private func f(_ id: String, _ title: String = "") -> DoctorViewModel.Finding {
        var x = DoctorViewModel.Finding(); x.id = id; x.title = title; return x
    }

    func testCategorizesAndOrders() {
        let secs = OnboardResult.sections([
            f("config.validate"),
            f("boot.api/web"),
            f("secret.missing:KEY"),
            f("setup.api"),
        ])
        // secrets lead, then boot, setup, config
        XCTAssertEqual(secs.map(\.cat), [.secret, .boot, .setup, .config])
    }

    func testDropsEmptyGroupsAndGroupsMultiple() {
        let secs = OnboardResult.sections([f("boot.a/x"), f("boot.a/y")])
        XCTAssertEqual(secs.count, 1)
        XCTAssertEqual(secs.first?.cat, .boot)
        XCTAssertEqual(secs.first?.findings.count, 2)
    }
}
