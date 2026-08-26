import XCTest
@testable import PomeloApp

final class ConfigTreeTests: XCTestCase {
    func testRootFileAndNestedFragments() {
        let nodes = ConfigTree.build([
            .init(rel: "pom.yml", path: "/p/pom.yml"),
            .init(rel: "pom.d/services.yml", path: "/p/pom.d/services.yml"),
            .init(rel: "pom.d/backend/api.yml", path: "/p/pom.d/backend/api.yml"),
        ])
        // root: pom.yml (file) + pom.d (dir), files sorted before dirs
        XCTAssertEqual(nodes.map(\.name), ["pom.yml", "pom.d"])
        XCTAssertFalse(nodes[0].isDir)
        let pomd = nodes[1]
        XCTAssertTrue(pomd.isDir)
        // pom.d children: services.yml (file) + backend (dir)
        XCTAssertEqual(pomd.children?.map(\.name), ["services.yml", "backend"])
        let backend = pomd.children!.first { $0.isDir }!
        XCTAssertEqual(backend.children?.map(\.name), ["api.yml"])
        XCTAssertEqual(backend.children?.first?.path, "/p/pom.d/backend/api.yml")
    }

    func testFlatIsUnchanged() {
        let nodes = ConfigTree.build([.init(rel: "pom.yml", path: "/p/pom.yml")])
        XCTAssertEqual(nodes.count, 1)
        XCTAssertEqual(nodes[0].path, "/p/pom.yml")
    }
}
