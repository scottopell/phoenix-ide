import XCTest

@testable import PhoenixMobile

final class CertPinStoreTests: XCTestCase {
    private var suiteName = ""
    private var defaults: UserDefaults!

    override func setUp() {
        super.setUp()
        suiteName = "PhoenixMobile.CertPinStoreTests.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suiteName)
        defaults = nil
        super.tearDown()
    }

    func testExistingPinIsCheckedBeforeAnyTrustShortcut() {
        XCTAssertEqual(
            CertPinStore.evaluateExisting(
                host: "phoenix.local", port: 8031, fingerprint: "a", defaults: defaults),
            .unpinned)
        XCTAssertEqual(
            CertPinStore.evaluate(
                host: "phoenix.local", port: 8031, fingerprint: "a", defaults: defaults),
            .accept)
        XCTAssertEqual(
            CertPinStore.evaluateExisting(
                host: "phoenix.local", port: 8031, fingerprint: "a", defaults: defaults),
            .accept)
        XCTAssertEqual(
            CertPinStore.evaluateExisting(
                host: "phoenix.local", port: 8031, fingerprint: "b", defaults: defaults),
            .reject)
        XCTAssertNotNil(CertPinStore.lastMismatchAt(in: defaults))
    }
}
