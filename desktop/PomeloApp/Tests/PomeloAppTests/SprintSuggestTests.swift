import XCTest
@testable import PomeloApp

final class SprintSuggestTests: XCTestCase {
    private func iss(_ key: String, _ summary: String = "", mine: Bool = false) -> SprintIssue {
        SprintIssue(key: key, summary: summary, mine: mine)
    }

    func testExcludesExistingBranches() {
        let out = SprintSuggest.rank([iss("PROJ-1"), iss("PROJ-2")],
                                     existing: ["PROJ-1"], query: "", onlyMine: false)
        XCTAssertEqual(out.map(\.key), ["PROJ-2"])
    }

    func testOnlyMineFilter() {
        let out = SprintSuggest.rank([iss("PROJ-1", mine: true), iss("PROJ-2", mine: false)],
                                     existing: [], query: "", onlyMine: true)
        XCTAssertEqual(out.map(\.key), ["PROJ-1"])
    }

    func testMineSortedFirstWhenNoQuery() {
        let out = SprintSuggest.rank([iss("PROJ-2", mine: false), iss("PROJ-1", mine: true)],
                                     existing: [], query: "", onlyMine: false)
        XCTAssertEqual(out.map(\.key), ["PROJ-1", "PROJ-2"])
    }

    func testQueryDropsNonMatches() {
        let out = SprintSuggest.rank([iss("PROJ-1", "add login"), iss("PROJ-2", "fix payments")],
                                     existing: [], query: "payments", onlyMine: false)
        XCTAssertEqual(out.map(\.key), ["PROJ-2"])
    }
}
