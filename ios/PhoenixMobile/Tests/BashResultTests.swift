import XCTest

@testable import PhoenixMobile

final class BashResultTests: XCTestCase {
    func testKilledTombstoneUsesFinalCauseWithoutSignalNumber() throws {
        let result = try XCTUnwrap(BashResult(
            resultText: "{\"status\":\"tombstoned\",\"final_cause\":\"killed\",\"exit_code\":null,\"duration_ms\":1200,\"lines\":[]}"))

        XCTAssertTrue(result.isFailure)
        XCTAssertEqual(result.headline, "killed · 1.2s")
    }

    func testExitedTombstoneUsesUnderlyingExitCause() throws {
        let result = try XCTUnwrap(BashResult(
            resultText: "{\"status\":\"tombstoned\",\"final_cause\":\"exited\",\"exit_code\":0,\"duration_ms\":50,\"lines\":[]}"))

        XCTAssertFalse(result.isFailure)
        XCTAssertEqual(result.headline, "exited 0 · 50ms")
    }
}
