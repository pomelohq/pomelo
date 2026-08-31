import XCTest
import SwiftUI
@testable import PomeloApp

// The PR row badges and the detail header wrap instead of clipping once the
// workspace split is dragged narrow, so the packing has to be exact at the edges.
final class WrapRowTests: XCTestCase {
    private func sizes(_ widths: [CGFloat], height: CGFloat = 14) -> [CGSize] {
        widths.map { CGSize(width: $0, height: height) }
    }

    func testEverythingOnOneRowWhenItFits() {
        let rows = WrapRow.pack(sizes([40, 40, 40]), maxW: 200, spacing: 5)
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].indices, [0, 1, 2])
        XCTAssertEqual(rows[0].width, 130)  // 40*3 + 5*2
    }

    func testOverflowStartsANewRow() {
        let rows = WrapRow.pack(sizes([60, 60, 60]), maxW: 130, spacing: 5)
        XCTAssertEqual(rows.map(\.indices), [[0, 1], [2]])
    }

    // Spacing counts toward the fit: two 60s plus a 5 gap is 125, not 120.
    func testSpacingCountsTowardTheFit() {
        XCTAssertEqual(WrapRow.pack(sizes([60, 60]), maxW: 124, spacing: 5).count, 2)
        XCTAssertEqual(WrapRow.pack(sizes([60, 60]), maxW: 125, spacing: 5).count, 1)
    }

    // A badge wider than the column must still be placed, not silently dropped.
    func testOversizedItemStillGetsARow() {
        let rows = WrapRow.pack(sizes([400]), maxW: 100, spacing: 5)
        XCTAssertEqual(rows.map(\.indices), [[0]])
    }

    func testOversizedItemDoesNotSwallowFollowingItems() {
        let rows = WrapRow.pack(sizes([400, 20]), maxW: 100, spacing: 5)
        XCTAssertEqual(rows.map(\.indices), [[0], [1]])
    }

    func testRowHeightIsTheTallestItem() {
        let mixed = [CGSize(width: 20, height: 10), CGSize(width: 20, height: 26)]
        XCTAssertEqual(WrapRow.pack(mixed, maxW: 200, spacing: 5).first?.height, 26)
    }

    func testNoItemsMeansNoRows() {
        XCTAssertTrue(WrapRow.pack([], maxW: 200, spacing: 5).isEmpty)
    }

    // The four open-PR badges (draft/CI/review/conflict) are the real worst case.
    func testFourBadgesWrapInsteadOfClippingInANarrowMaster() {
        let badges = sizes([46, 38, 62, 56])
        XCTAssertEqual(WrapRow.pack(badges, maxW: 320, spacing: 5).count, 1)
        let narrow = WrapRow.pack(badges, maxW: 150, spacing: 5)
        XCTAssertGreaterThan(narrow.count, 1)
        XCTAssertEqual(narrow.flatMap(\.indices), [0, 1, 2, 3])
        XCTAssertTrue(narrow.allSatisfy { $0.width <= 150 })
    }
}
