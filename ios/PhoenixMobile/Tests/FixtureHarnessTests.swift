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

    func testNormalFixtureCoversNativeMessageFamilies() {
        let types = Set(FixtureScenario.scenario(for: .normal).screen.messages.map(\.message_type))

        XCTAssertTrue(["user", "agent", "tool", "skill", "system", "continuation"].allSatisfy(types.contains))
        let sequences = FixtureScenario.scenario(for: .normal).screen.messages.map(\.sequence_id)
        XCTAssertEqual(sequences, Array(1...Int64(sequences.count)))
    }

    func testNormalFixtureShowsVerifiedAndUnverifiedInputWithoutInferringSenderFromRole() {
        let messages = FixtureScenario.scenario(for: .normal).screen.messages
        let byId = Dictionary(uniqueKeysWithValues: messages.map { ($0.message_id, $0) })
        XCTAssertEqual(byId["m-user-1"]?.inputOrigin, .userApi)
        XCTAssertEqual(
            byId["m-internal-input"]?.inputOrigin,
            .internalConversation(productConversationId: "pc-parent", transcriptId: "parent-row"))
        XCTAssertEqual(byId["m-historical-input"]?.inputOrigin, .unknownHistorical)
        XCTAssertEqual(byId["m-skill"]?.inputOrigin, byId["m-internal-input"]?.inputOrigin)
        XCTAssertFalse(byId["m-skill"]?.inputOrigin.isHumanInput ?? true)
        XCTAssertEqual(byId["m-skill"]?.content["trigger"]?.stringValue, "/phoenix-development")
    }

    func testQueuedFixtureEntriesRemainLocalAndUnattributedUntilAuthoritativeHistory() {
        let entries = FixtureScenario.outboxEntries
        XCTAssertEqual(Set(entries.map(\.status)), [
            .pending, .steeringQueued, .failed, .recoverableInconsistency,
        ])
        XCTAssertTrue(entries.allSatisfy { $0.isVisible && $0.conversationId == "fixture-conv" })
        XCTAssertTrue(entries.allSatisfy { entry in
            !FixtureScenario.scenario(for: .normal).screen.messages.contains {
                $0.message_id == entry.localId
            }
        })
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
