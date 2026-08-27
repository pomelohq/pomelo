import XCTest
@testable import PomeloApp

@MainActor
final class NMStoreViewModelTests: XCTestCase {
    private let json = #"{"entries":[{"repo":"api","hash":"aaa","bytes":1073741824,"current":true,"orphan":false,"consumers":[{"branch":"main","is_main":true}]},{"repo":"api","hash":"bbb","bytes":536870912,"current":false,"orphan":true,"consumers":[]},{"repo":"web","hash":"ccc","bytes":104857600,"current":false,"orphan":true,"consumers":[]}],"total":1715470336,"unoptimized":[]}"#

    func testStaleAndHuman() async {
        let mock = MockPomAPI(); mock.nmStoreJSON = json
        let vm = NMStoreViewModel(api: mock)
        await vm.load()

        XCTAssertEqual(vm.entries.count, 3)
        XCTAssertEqual(vm.stale.count, 2, "entries with no consumers are stale")
        XCTAssertEqual(vm.staleBytes, 536870912 + 104857600)
        XCTAssertEqual(vm.human(1073741824), "1.00 GB")
        XCTAssertEqual(vm.human(104857600), "100 MB")
    }

    func testDeleteStaleRemovesOnlyOrphans() async {
        let mock = MockPomAPI(); mock.nmStoreJSON = json
        let vm = NMStoreViewModel(api: mock)
        await vm.load()
        await vm.deleteStale()

        XCTAssertEqual(mock.nmDeleted.sorted(), ["api/bbb", "web/ccc"])
        XCTAssertFalse(mock.nmDeleted.contains("api/aaa"), "in-use entry is never deleted")
    }
}
