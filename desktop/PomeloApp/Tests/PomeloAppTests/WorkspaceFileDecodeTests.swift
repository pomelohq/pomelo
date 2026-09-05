import XCTest
@testable import PomeloApp

final class WorkspaceFileDecodeTests: XCTestCase {
    func testDecodesEntryWithoutSizeKey() throws {
        let json = Data(#"[{"repo":"api","path":"src","is_dir":true}]"#.utf8)
        let entries = try XCTUnwrap(PomJSON.decode([WorkspaceFileEntry].self, from: json))
        XCTAssertEqual(entries.count, 1)
        XCTAssertEqual(entries[0].path, "src")
        XCTAssertTrue(entries[0].isDir)
        XCTAssertEqual(entries[0].size, 0)
    }
}
