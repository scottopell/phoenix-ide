import XCTest
@testable import PhoenixMobile

final class RenderingReducerTests: XCTestCase {
    func testDisplayDataPatchPreservesExistingMetadataAndToolStarts() {
        let existing: JSONValue = .object([
            "command": .string("cargo test"),
            "tool_starts": .object(["first": .number(1)]),
        ])
        let patch: JSONValue = .object([
            "tool_starts": .object(["second": .number(2)]),
        ])

        XCTAssertEqual(
            ConversationSession.mergeDisplayData(existing: existing, patch: patch),
            .object([
                "command": .string("cargo test"),
                "tool_starts": .object([
                    "first": .number(1),
                    "second": .number(2),
                ]),
            ]))
    }

    func testKilledTombstoneWithoutSignalIsFailure() throws {
        let result = try XCTUnwrap(BashResult(
            resultText: #"{"status":"tombstoned","final_cause":"killed"}"#))

        XCTAssertTrue(result.isFailure)
        XCTAssertEqual(result.headline, "killed")
    }

    func testAuthenticatedAPIRequiresHTTPS() throws {
        let httpURL = try XCTUnwrap(URL(string: "http://phoenix.local:8031"))
        let httpsURL = try XCTUnwrap(URL(string: "https://phoenix.local:8031"))

        XCTAssertNil(PhoenixAPI(baseURL: httpURL, password: "secret", allowSelfSigned: true))
        XCTAssertNotNil(PhoenixAPI(baseURL: httpURL, password: nil, allowSelfSigned: true))
        XCTAssertNotNil(PhoenixAPI(baseURL: httpsURL, password: "secret", allowSelfSigned: true))
    }

    func testExistingCertificatePinAppliesIndependentlyOfChainTrust() throws {
        let suite = "phoenix-cert-test-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        XCTAssertEqual(
            CertPinStore.evaluate(
                host: "phoenix.local", port: 443, fingerprint: "leaf-a", defaults: defaults),
            .accept)
        XCTAssertEqual(
            CertPinStore.evaluateExisting(
                host: "phoenix.local", port: 443, fingerprint: "leaf-b", defaults: defaults),
            .reject)
        XCTAssertNotNil(CertPinStore.lastMismatchAt(in: defaults))
        XCTAssertEqual(
            CertPinStore.evaluateExisting(
                host: "phoenix.local", port: 443, fingerprint: "leaf-a", defaults: defaults),
            .accept)
        XCTAssertNil(CertPinStore.lastMismatchAt(in: defaults))
    }
}
