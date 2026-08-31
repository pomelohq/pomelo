import XCTest
@testable import PomeloApp

@MainActor
final class GitViewModelTests: XCTestCase {
    private let json = #"""
    {"repos":[
      {"repo":"api","branch":"feat","ahead":2,"behind":0,"changes":[
        {"path":"a.go","index":"M","worktree":" ","staged":true,"unstaged":false,"untracked":false},
        {"path":"b.go","index":" ","worktree":"M","staged":false,"unstaged":true,"untracked":false},
        {"path":"new.txt","index":"?","worktree":"?","staged":false,"unstaged":true,"untracked":true}
      ]},
      {"repo":"web","branch":"feat","ahead":0,"behind":0,"changes":[]}
    ]}
    """#

    func testGroupingAndCounts() async {
        let mock = MockPomAPI(); mock.gitStatusJSON = json
        let vm = GitViewModel(branch: "feat", isMain: false, api: mock)
        await vm.load()

        XCTAssertEqual(vm.repos.count, 2)
        XCTAssertEqual(vm.totalChanges, 3)
        let api = vm.repos[0]
        XCTAssertEqual(api.staged.map(\.path), ["a.go"])
        XCTAssertEqual(api.unstaged.map(\.path), ["b.go", "new.txt"])
        XCTAssertTrue(vm.repos[1].isClean)
        XCTAssertEqual(api.changes.first(where: { $0.untracked })?.badge, "U")
    }

    func testStageUnstageDiscardForwardPaths() async {
        let mock = MockPomAPI(); mock.gitStatusJSON = json
        let vm = GitViewModel(branch: "feat", isMain: false, api: mock)
        await vm.load()

        await vm.stage("api", ["b.go"])
        await vm.unstage("api", ["a.go"])
        await vm.discard("api", ["new.txt"])
        await vm.push("api")

        XCTAssertEqual(mock.gitCalls.count, 4)
        XCTAssertEqual(mock.gitCalls[0].0, "stage")
        XCTAssertEqual(mock.gitCalls[0].2, ["b.go"])
        XCTAssertEqual(mock.gitCalls[1].0, "unstage")
        XCTAssertEqual(mock.gitCalls[3].0, "push")
    }

    func testCommitRequiresMessage() async {
        let mock = MockPomAPI(); mock.gitStatusJSON = json
        let vm = GitViewModel(branch: "feat", isMain: false, api: mock)
        await vm.load()

        await vm.commit("api")
        XCTAssertTrue(mock.gitCommitCalls.isEmpty, "empty message must not commit")
        XCTAssertFalse(vm.lastError.isEmpty)

        vm.commitMessage["api"] = "fix things"
        await vm.commit("api")
        XCTAssertEqual(mock.gitCommitCalls.first?.0, "api")
        XCTAssertEqual(mock.gitCommitCalls.first?.1, "fix things")
        XCTAssertEqual(vm.commitMessage["api"], "", "message clears after a successful commit")
    }
}
