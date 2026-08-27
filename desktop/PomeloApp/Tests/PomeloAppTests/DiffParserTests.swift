import XCTest
@testable import PomeloApp

// Diff PARSING now lives in Go (internal/diffparse) — see its _test.go for the
// format cases. These cover the rename-display derivation that stays FE (ADR 0001:
// how to present a rename compactly is UI/UX) and decoding the Go payload.
final class DiffModelTests: XCTestCase {
    func testDecodesGoPayload() {
        let json = #"[{"path":"src/new.ts","old_path":"src/old.ts","status":"R","adds":1,"dels":0,"binary":false,"header_old_path":"src/old.ts","lines":[{"id":1,"kind":"hunk","text":"@@ -1,2 +1,3 @@"},{"id":2,"kind":"add","new_n":3,"text":"extra"}]}]"#
        let files = PomJSON.decode([DiffFile].self, from: Data(json.utf8))
        XCTAssertEqual(files?.count, 1)
        let f = files?[0]
        XCTAssertEqual(f?.status, "R")
        XCTAssertEqual(f?.path, "src/new.ts")
        XCTAssertEqual(f?.oldPath, "src/old.ts")
        XCTAssertEqual(f?.adds, 1)
        XCTAssertEqual(f?.lines.count, 2)
        XCTAssertEqual(f?.lines.first?.kind, .hunk)
        XCTAssertEqual(f?.lines.last?.newN, 3)
    }

    func testRenamePartsSharesCommonPrefix() {
        var f = DiffFile(path: "deep/nested/dir/g.ts", oldPath: "deep/nested/dir/f.ts", status: "R")
        guard let r = f.renameParts else { return XCTFail("expected rename parts") }
        XCTAssertEqual(r.prefix, "deep/nested/dir/")
        XCTAssertEqual(r.from, "f.ts")
        XCTAssertEqual(r.to, "g.ts")
        f.status = "M"
        XCTAssertNil(f.renameParts, "non-rename has no rename parts")
    }

    func testRenameAcrossDirsKeepsDivergentSegments() {
        let f = DiffFile(path: "apps/portal/hooks/use-copy.ts", oldPath: "apps/portal/ui/CopyButton.tsx", status: "R")
        guard let r = f.renameParts else { return XCTFail("expected rename parts") }
        XCTAssertEqual(r.prefix, "apps/portal/")
        XCTAssertEqual(r.from, "ui/CopyButton.tsx")
        XCTAssertEqual(r.to, "hooks/use-copy.ts")
    }

    func testPlainModificationHasNoRenameParts() {
        let f = DiffFile(path: "x.ts", oldPath: nil, status: "M")
        XCTAssertNil(f.renameParts)
    }
}

final class FileTreeBuilderTests: XCTestCase {
    private func tree(_ paths: [String]) -> [FileTreeNode] {
        FileTreeBuilder.build(paths.map { DiffFile(path: $0, oldPath: nil, status: "M") })
    }

    func testSharedRootPrefixBecomesItsOwnRow() {
        let t = tree([
            "apps/portal/src/providers/Routes/main.ts",
            "apps/portal/src/shared/hooks/use-copy.ts",
        ])
        XCTAssertEqual(t.count, 1)
        XCTAssertEqual(t[0].name, "apps/portal/src", "root prefix must not be swallowed by the root node")
        XCTAssertEqual(t[0].id, "apps/portal/src")
    }

    func testSingleChildChainCollapsesKeepingFullID() {
        let t = tree([
            "a/b/c/one.ts",
            "a/b/c/two.ts",
        ])
        XCTAssertEqual(t[0].name, "a/b/c")
        XCTAssertEqual(t[0].id, "a/b/c", "id stays the full path — it keys collapse state")
        XCTAssertEqual(t[0].children.map(\.name), ["one.ts", "two.ts"])
    }

    func testFolderWithOneFileStaysAFolder() {
        let t = tree(["routes/active/main-routes.ts", "routes/redirects.tsx"])
        XCTAssertEqual(t[0].name, "routes")
        let active = t[0].children.first { !$0.isLeaf }
        XCTAssertEqual(active?.name, "active")
        XCTAssertEqual(active?.children.map(\.name), ["main-routes.ts"])
    }

    func testFoldersSortBeforeFiles() {
        let t = tree(["x/zeta/a.ts", "x/alpha.ts"])
        XCTAssertEqual(t[0].children.map(\.isLeaf), [false, true])
    }

    func testSingleTopLevelFile() {
        let t = tree(["README.md"])
        XCTAssertEqual(t.count, 1)
        XCTAssertTrue(t[0].isLeaf)
        XCTAssertEqual(t[0].name, "README.md")
    }
}
