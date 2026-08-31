#if DEBUG
import XCTest
@testable import PhoenixMobile

final class FixtureHarnessTests: XCTestCase {
    func testFixtureLaunchSelectionParsesKnownScenario() {
        XCTAssertEqual(
            FixtureAppLaunch.selection(from: ["PhoenixMobile", "-fixture", "offline"]),
            .offline)
    }

    func testFixtureLaunchSelectionRejectsMissingOrUnknownScenario() {
        XCTAssertNil(FixtureAppLaunch.selection(from: ["PhoenixMobile"]))
        XCTAssertNil(FixtureAppLaunch.selection(from: ["PhoenixMobile", "-fixture"]))
        XCTAssertNil(FixtureAppLaunch.selection(from: ["PhoenixMobile", "-fixture", "bogus"]))
    }

    func testFixtureRequestRemainsIsolatedWhenIdentifierIsInvalid() {
        let arguments = ["PhoenixMobile", "-fixture", "bogus"]

        XCTAssertTrue(FixtureAppLaunch.isRequested(in: arguments))
        XCTAssertNil(FixtureAppLaunch.selection(from: arguments))
    }

    func testFixtureCatalogCoversExpectedScenarioSet() {
        XCTAssertEqual(
            Set(FixtureScenario.all.map(\.id)),
            Set(FixtureScenario.ID.allCases))
    }

    func testNormalFixtureCoversValidAndMalformedImages() {
        let images = FixtureScenario.scenario(for: .normal).screen.messages[0].content["images"]?.arrayValue

        XCTAssertEqual(images?.count, 2)
        XCTAssertNotNil(Data(base64Encoded: images?[0]["data"]?.stringValue ?? ""))
        XCTAssertNil(Data(base64Encoded: images?[1]["data"]?.stringValue ?? ""))
    }

    func testNormalFixtureUsesShippedToolExecutingPayloadShape() {
        let state = ConversationState.parse(FixtureScenario.scenario(for: .normal).screen.statePayload)

        guard case .toolExecuting(let name, let remaining, let completed) = state else {
            return XCTFail("expected tool_executing state")
        }
        XCTAssertEqual(name, "bash")
        XCTAssertEqual(remaining, 1)
        XCTAssertEqual(completed, 2)
    }

    func testEveryFixtureUsesFixedMessageIdentityAndTimestamp() {
        for scenario in FixtureScenario.all {
            for message in scenario.screen.messages {
                XCTAssertTrue(message.message_id.hasPrefix("m-"), scenario.id.rawValue)
                XCTAssertEqual(message.conversation_id, "fixture-conv", scenario.id.rawValue)
                XCTAssertNotNil(message.created_at, scenario.id.rawValue)
            }
        }
    }

    func testMalformedScenarioIncludesVisibleFallbackToolAndMessage() {
        let scenario = FixtureScenario.scenario(for: .malformed)

        XCTAssertTrue(scenario.screen.messages.contains { $0.message_type == "tool" })
        XCTAssertEqual(scenario.screen.toolIndex["tool-unknown-1"]?.name, "future_tool")
    }

    func testOfflineScenarioStateRemainsNonActionable() {
        let scenario = FixtureScenario.scenario(for: .offline)

        XCTAssertFalse(scenario.screen.isOnline)
        XCTAssertEqual(scenario.screen.presentationMode, "needs_action")
        XCTAssertTrue(scenario.screen.requiresAction)
    }
}
#endif
