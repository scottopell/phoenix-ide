import XCTest

@testable import PhoenixMobile

final class CertPinStoreTests: XCTestCase {
    override func setUp() {
        super.setUp()
        CertPinStore.forget()
    }

    override func tearDown() {
        CertPinStore.forget()
        super.tearDown()
    }

    func testExistingPinIsCheckedBeforeAnyTrustShortcut() {
        XCTAssertEqual(
            CertPinStore.evaluateExisting(host: "phoenix.local", port: 8031, fingerprint: "a"),
            .unpinned)
        XCTAssertEqual(
            CertPinStore.evaluate(host: "phoenix.local", port: 8031, fingerprint: "a"),
            .accept)
        XCTAssertEqual(
            CertPinStore.evaluateExisting(host: "phoenix.local", port: 8031, fingerprint: "a"),
            .accept)
        XCTAssertEqual(
            CertPinStore.evaluateExisting(host: "phoenix.local", port: 8031, fingerprint: "b"),
            .reject)
        XCTAssertNotNil(CertPinStore.lastMismatchAt)
    }
}
