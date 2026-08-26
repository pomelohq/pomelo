import XCTest
@testable import PomeloApp

final class SecretsViewModelTests: XCTestCase {
    @MainActor
    func testLoadAddRemove() async {
        let mock = MockPomAPI()
        mock.secretNamesJSON = #"{"names":["B","A"]}"#
        let vm = SecretsViewModel(api: mock)

        await vm.load()
        XCTAssertEqual(vm.names, ["A", "B"])  // sorted

        vm.newName = "STRIPE_KEY"; vm.newValue = "sk_test"
        await vm.add()
        XCTAssertTrue(mock.secretSetCalls.contains { $0.0 == "STRIPE_KEY" && $0.1 == "sk_test" })
        XCTAssertEqual(vm.newName, "")  // cleared after add

        await vm.remove("A")
        XCTAssertTrue(mock.secretSetCalls.contains { $0.0 == "A" && $0.1 == "" })  // empty value = delete
    }

    @MainActor
    func testAddIgnoresBlank() async {
        let mock = MockPomAPI()
        let vm = SecretsViewModel(api: mock)
        vm.newName = "  "; vm.newValue = ""
        await vm.add()
        XCTAssertTrue(mock.secretSetCalls.isEmpty)
    }
}
