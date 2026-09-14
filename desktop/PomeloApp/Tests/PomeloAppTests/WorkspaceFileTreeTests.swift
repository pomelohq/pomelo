import XCTest
@testable import PomeloApp

final class WorkspaceFileTreeTests: XCTestCase {
    func testGroupsByRepoAndNestsPathsUnderRoot() {
        let entries: [WorkspaceFileEntry] = [
            .init(repo: "api", path: "main.go", isDir: false),
            .init(repo: "api", path: "src", isDir: true),
            .init(repo: "api", path: "src/a.go", isDir: false),
            .init(repo: "web", path: "index.ts", isDir: false),
        ]
        let roots = WFileTreeBuilder.build(entries, rootName: "ws")
        XCTAssertEqual(roots.count, 1)
        let root = roots[0]
        XCTAssertEqual(root.name, "ws")
        XCTAssertTrue(root.isRoot)
        XCTAssertEqual(root.children.map(\.name), ["api", "web"])

        let api = root.children[0]
        XCTAssertEqual(api.children.map(\.name), ["src", "main.go"])
        let src = api.children.first { $0.name == "src" }!
        XCTAssertFalse(src.isLeaf)
        XCTAssertEqual(src.children.map(\.name), ["a.go"])
        XCTAssertTrue(src.children[0].isLeaf)

        let mainGo = api.children.first { $0.name == "main.go" }!
        XCTAssertTrue(mainGo.isLeaf)
        XCTAssertEqual(mainGo.entry?.path, "main.go")
    }

    func testEmptyEntriesProduceRootWithNoChildren() {
        let roots = WFileTreeBuilder.build([], rootName: "ws")
        XCTAssertEqual(roots.count, 1)
        XCTAssertTrue(roots[0].children.isEmpty)
    }

    func testRootLevelFilesSitAtTopLevelAfterRepos() {
        let entries: [WorkspaceFileEntry] = [
            .init(repo: "api", path: "main.go", isDir: false),
            .init(repo: "", path: "CLAUDE.md", isDir: false),
            .init(repo: "", path: ".gitignore", isDir: false),
        ]
        let root = WFileTreeBuilder.build(entries, rootName: "ws")[0]
        XCTAssertEqual(root.children.map(\.name), ["api", ".gitignore", "CLAUDE.md"])

        let claude = root.children.first { $0.name == "CLAUDE.md" }!
        XCTAssertTrue(claude.isLeaf)
        XCTAssertTrue(claude.children.isEmpty)
        XCTAssertEqual(claude.entry?.repo, "")
        XCTAssertEqual(claude.entry?.path, "CLAUDE.md")
    }
}
