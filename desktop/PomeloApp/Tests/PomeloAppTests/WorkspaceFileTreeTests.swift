import XCTest
@testable import PomeloApp

final class WorkspaceFileTreeTests: XCTestCase {
    func testGroupsByRepoAndNestsPaths() {
        let entries: [WorkspaceFileEntry] = [
            .init(repo: "api", path: "main.go", isDir: false),
            .init(repo: "api", path: "src", isDir: true),
            .init(repo: "api", path: "src/a.go", isDir: false),
            .init(repo: "web", path: "index.ts", isDir: false),
        ]
        let roots = WFileTreeBuilder.build(entries)
        XCTAssertEqual(roots.map(\.name), ["api", "web"])

        let api = roots[0]
        XCTAssertEqual(api.children.map(\.name), ["src", "main.go"])
        let src = api.children.first { $0.name == "src" }!
        XCTAssertFalse(src.isLeaf)
        XCTAssertEqual(src.children.map(\.name), ["a.go"])
        XCTAssertTrue(src.children[0].isLeaf)

        let mainGo = api.children.first { $0.name == "main.go" }!
        XCTAssertTrue(mainGo.isLeaf)
        XCTAssertEqual(mainGo.entry?.path, "main.go")
    }

    func testEmptyEntriesProduceNoRoots() {
        XCTAssertTrue(WFileTreeBuilder.build([]).isEmpty)
    }

    func testRootLevelFilesSitAtTopLevelAfterRepos() {
        let entries: [WorkspaceFileEntry] = [
            .init(repo: "api", path: "main.go", isDir: false),
            .init(repo: "", path: "CLAUDE.md", isDir: false),
            .init(repo: "", path: ".gitignore", isDir: false),
        ]
        let roots = WFileTreeBuilder.build(entries)
        XCTAssertEqual(roots.map(\.name), ["api", ".gitignore", "CLAUDE.md"])

        let claude = roots.first { $0.name == "CLAUDE.md" }!
        XCTAssertTrue(claude.isLeaf)
        XCTAssertTrue(claude.children.isEmpty)
        XCTAssertEqual(claude.entry?.repo, "")
        XCTAssertEqual(claude.entry?.path, "CLAUDE.md")
    }
}
