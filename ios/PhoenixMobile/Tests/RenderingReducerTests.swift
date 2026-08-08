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
}
