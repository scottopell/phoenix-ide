import XCTest
@testable import PhoenixMobile

final class RenderingReducerTests: XCTestCase {
    func testKnownNoteShapesUseUserFacingFields() {
        XCTAssertEqual(
            MessageView.noteText(
                messageType: "continuation",
                content: .object(["summary": .string("Continue from here")])),
            "Continue from here")
        XCTAssertEqual(
            MessageView.noteText(
                messageType: "skill",
                content: .object([
                    "name": .string("build"),
                    "body": .string("expanded prompt"),
                    "trigger": .string("/build"),
                ])),
            "/build")
        XCTAssertEqual(
            MessageView.noteText(
                messageType: "error",
                content: .object(["message": .string("Something failed")])),
            "Something failed")
    }

    func testMessageOriginDecodingAndCachedHistoricalFallback() throws {
        let cases: [(String, InputOrigin, String)] = [
            (#"{"kind":"internal_conversation","product_conversation_id":"product-1","transcript_id":"transcript-1","source_call":{"message_id":"source-message","tool_use_id":"source-tool"}}"#, .internalConversation(productConversationId: "product-1", transcriptId: "transcript-1", sourceCall: SourceToolCall(message_id: "source-message", tool_use_id: "source-tool")), "Conversation from @transcript:transcript-1 (conversation ID product-1)"),
            (#"{"kind":"user_api"}"#, .userApi, "User API"),
            (#"{"kind":"internal_conversation","product_conversation_id":"product-1","transcript_id":"transcript-1"}"#,
             .internalConversation(productConversationId: "product-1", transcriptId: "transcript-1"),
             "Conversation from @transcript:transcript-1 (conversation ID product-1)"),
            (#"{"kind":"system_generated"}"#, .systemGenerated, "System input"),
            (#"{"kind":"subscription_event","event_id":"event-1"}"#,
             .subscriptionEvent(eventId: "event-1"), "Conversation event"),
            (#"{"kind":"unknown_historical"}"#, .unknownHistorical, "Unknown input"),
        ]
        let base = #"{"message_id":"m1","sequence_id":1,"message_type":"user","content":{"text":"hello"}"#
        let historical = try JSONDecoder().decode(Message.self, from: Data((base + "}").utf8))
        XCTAssertEqual(historical.inputOrigin, .unknownHistorical)
        XCTAssertEqual(historical.inputOrigin.label, "Unknown input")
        for (json, expected, label) in cases {
            let raw = base + ",\"origin\":" + json + "}"
            let decoded = try JSONDecoder().decode(Message.self, from: Data(raw.utf8))
            XCTAssertEqual(decoded.inputOrigin, expected)
            XCTAssertEqual(decoded.inputOrigin.label, label)
            let roundTrip = try JSONDecoder().decode(Message.self, from: JSONEncoder().encode(decoded))
            XCTAssertEqual(roundTrip.inputOrigin, expected)
        }
    }

    func testOnlyUserApiOriginIsHumanAcrossMessageRolesAndCacheRoundTrip() throws {
        let origins: [(InputOrigin, Bool)] = [
            (.userApi, true),
            (.internalConversation(productConversationId: "pc-parent", transcriptId: "row-parent"), false),
            (.unknownHistorical, false),
            (.systemGenerated, false),
            (.subscriptionEvent(eventId: "event-1"), false),
        ]
        for (origin, isHuman) in origins {
            XCTAssertEqual(origin.isUserApiInput, isHuman)
            for type in ["user", "skill", "agent"] {
                let message = Message(
                    message_id: "m-\(type)", conversation_id: "c1", sequence_id: 1,
                    message_type: type, content: .object(["text": .string("same text")]),
                    display_data: nil, created_at: nil, origin: origin)
                let restored = try JSONDecoder().decode(Message.self, from: JSONEncoder().encode(message))
                XCTAssertEqual(restored.inputOrigin.isUserApiInput, isHuman, type)
            }
        }
    }

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
