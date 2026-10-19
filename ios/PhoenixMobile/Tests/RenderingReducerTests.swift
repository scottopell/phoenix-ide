import XCTest
@testable import PhoenixMobile

final class RenderingReducerTests: XCTestCase {
    func testSourceCallLinkPinsOriginalToolAndHistoricalAbsenceStaysUnavailable() throws {
        let origin = InputOrigin.internalConversation(productConversationId: "product", transcriptId: "original-member", sourceCall: SourceToolCall(message_id: "assistant-message", tool_use_id: "send-tool"))
        let url = try XCTUnwrap(origin.sourceCallURL(serverURL: "https://example.test"))
        XCTAssertEqual(url.path, "/c/original-member")
        XCTAssertEqual(url.fragment, "message-assistant-message")
        XCTAssertTrue(url.query?.contains("source_tool=send-tool") == true)
        XCTAssertFalse(origin.sourceCallUnavailable)
        let historical = InputOrigin.internalConversation(productConversationId: "product", transcriptId: "original-member")
        XCTAssertNil(historical.sourceCallURL(serverURL: "https://example.test"))
        XCTAssertTrue(historical.sourceCallUnavailable)
        XCTAssertEqual(historical.sourceTranscriptURL(serverURL: "https://example.test/phoenix")?.path, "/phoenix/c/original-member")
        XCTAssertEqual(origin.sourceCallURL(serverURL: "https://example.test/phoenix/")?.path, "/phoenix/c/original-member")
    }

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

    func testRemoteOriginDecodingEncodingAndHeaderLabelsPreserveExactSource() throws {
        let instanceText = "11111111-1111-4111-8111-111111111111"
        let instanceId = try XCTUnwrap(UUID(uuidString: instanceText))
        let sourceCall = SourceToolCall(message_id: "original-assistant", tool_use_id: "original-send")
        let cases: [(String, InputOrigin, String)] = [
            ("remote_api", .remoteApi(instanceId: instanceId), "Remote API from @instance:\(instanceText)"),
            (
                "remote_conversation",
                .remoteConversation(
                    instanceId: instanceId, productConversationId: "remote-product", transcriptId: "original-member",
                    sourceCall: sourceCall),
                "Remote conversation from @instance:\(instanceText) @transcript:original-member (conversation ID remote-product) — source message original-assistant, tool original-send"
            ),
            (
                "remote_coordinator",
                .remoteCoordinator(
                    instanceId: instanceId, productConversationId: "remote-product", transcriptId: "original-member",
                    sourceCall: sourceCall),
                "Remote coordinator from @instance:\(instanceText) @transcript:original-member (conversation ID remote-product) — source message original-assistant, tool original-send"
            ),
        ]
        for (kind, expected, label) in cases {
            var wire: [String: Any] = ["kind": kind, "instance_id": instanceText]
            if kind != "remote_api" {
                wire["product_conversation_id"] = "remote-product"
                wire["transcript_id"] = "original-member"
                wire["source_call"] = ["message_id": "original-assistant", "tool_use_id": "original-send"]
            }
            let decoded = try JSONDecoder().decode(InputOrigin.self, from: JSONSerialization.data(withJSONObject: wire))
            XCTAssertEqual(decoded, expected, kind)
            XCTAssertEqual(decoded.label, label, kind)
            XCTAssertFalse(decoded.isUserApiInput, kind)
            XCTAssertFalse(decoded.sourceCallUnavailable, kind)
            let encoded = try XCTUnwrap(
                JSONSerialization.jsonObject(with: JSONEncoder().encode(decoded)) as? NSDictionary)
            XCTAssertEqual(encoded, wire as NSDictionary, kind)
        }
    }

    func testRemoteOriginRequiresValidInstanceUUID() throws {
        for kind in ["remote_api", "remote_conversation", "remote_coordinator"] {
            for instance in [nil, NSNull(), "not-a-uuid", 123] as [Any?] {
                var wire: [String: Any] = [
                    "kind": kind, "product_conversation_id": "product", "transcript_id": "member",
                    "source_call": ["message_id": "assistant", "tool_use_id": "send"],
                ]
                wire["instance_id"] = instance
                let data = try JSONSerialization.data(withJSONObject: wire)
                XCTAssertThrowsError(try JSONDecoder().decode(InputOrigin.self, from: data), kind)
                let message: [String: Any] = [
                    "message_id": "m1", "sequence_id": 1, "message_type": "user",
                    "content": ["text": "hello"], "origin": wire,
                ]
                let messageData = try JSONSerialization.data(withJSONObject: message)
                XCTAssertThrowsError(try JSONDecoder().decode(Message.self, from: messageData), kind)
            }
        }
    }

    func testRemoteOriginsWithoutSourceCallRemainAttributedAndUnlinked() throws {
        let instanceText = "11111111-1111-4111-8111-111111111111"
        let instanceId = try XCTUnwrap(UUID(uuidString: instanceText))
        let cases: [(String, InputOrigin, String)] = [
            (
                "remote_conversation",
                .remoteConversation(instanceId: instanceId, productConversationId: "product", transcriptId: "member"),
                "Remote conversation"
            ),
            (
                "remote_coordinator",
                .remoteCoordinator(instanceId: instanceId, productConversationId: "product", transcriptId: "member"),
                "Remote coordinator"
            ),
        ]
        for (kind, expected, label) in cases {
            for sourceCall in [nil, NSNull()] as [Any?] {
                var wire: [String: Any] = [
                    "kind": kind, "instance_id": instanceText,
                    "product_conversation_id": "product", "transcript_id": "member",
                ]
                wire["source_call"] = sourceCall
                let decoded = try JSONDecoder().decode(
                    InputOrigin.self, from: JSONSerialization.data(withJSONObject: wire))
                XCTAssertEqual(decoded, expected)
                XCTAssertEqual(
                    decoded.label,
                    "\(label) from @instance:\(instanceText) @transcript:member (conversation ID product)")
                XCTAssertTrue(decoded.sourceCallUnavailable)
                XCTAssertNil(decoded.sourceTranscriptURL(serverURL: "https://receiver.test"))
                XCTAssertNil(decoded.sourceCallURL(serverURL: "https://receiver.test"))
                let restored = try JSONDecoder().decode(InputOrigin.self, from: JSONEncoder().encode(decoded))
                XCTAssertEqual(restored, expected)
            }
        }
    }

    func testRemoteAndLocalIDCollisionCannotCreateReceiverLocalLink() throws {
        let firstInstance = try XCTUnwrap(UUID(uuidString: "11111111-1111-4111-8111-111111111111"))
        let secondInstance = try XCTUnwrap(UUID(uuidString: "22222222-2222-4222-8222-222222222222"))
        let sourceCall = SourceToolCall(message_id: "same-assistant", tool_use_id: "same-tool")
        let local = InputOrigin.internalConversation(
            productConversationId: "same-product", transcriptId: "same-member", sourceCall: sourceCall)
        var labels: Set<String> = [local.label]
        for instance in [firstInstance, secondInstance] {
            let remoteOrigins: [InputOrigin] = [
                .remoteApi(instanceId: instance),
                .remoteConversation(
                    instanceId: instance, productConversationId: "same-product", transcriptId: "same-member",
                    sourceCall: sourceCall),
                .remoteCoordinator(
                    instanceId: instance, productConversationId: "same-product", transcriptId: "same-member",
                    sourceCall: sourceCall),
            ]
            for origin in remoteOrigins {
                XCTAssertNotEqual(origin, local)
                XCTAssertTrue(labels.insert(origin.label).inserted)
                for receiver in ["https://receiver.test", "https://receiver.test/phoenix/"] {
                    XCTAssertNil(origin.sourceTranscriptURL(serverURL: receiver))
                    XCTAssertNil(origin.sourceCallURL(serverURL: receiver))
                    XCTAssertEqual(local.sourceTranscriptURL(serverURL: receiver)?.lastPathComponent, "same-member")
                    XCTAssertEqual(local.sourceCallURL(serverURL: receiver)?.fragment, "message-same-assistant")
                }
            }
        }
    }

    func testOnlyUserApiOriginIsHumanAcrossMessageRolesAndCacheRoundTrip() throws {
        let instanceId = try XCTUnwrap(UUID(uuidString: "11111111-1111-4111-8111-111111111111"))
        let sourceCall = SourceToolCall(message_id: "remote-assistant", tool_use_id: "remote-send")
        let origins: [(InputOrigin, Bool)] = [
            (.userApi, true),
            (.remoteApi(instanceId: instanceId), false),
            (
                .remoteConversation(
                    instanceId: instanceId, productConversationId: "pc-parent", transcriptId: "row-parent",
                    sourceCall: sourceCall), false
            ),
            (
                .remoteCoordinator(
                    instanceId: instanceId, productConversationId: "pc-parent", transcriptId: "row-parent",
                    sourceCall: sourceCall), false
            ),
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
                XCTAssertEqual(restored.inputOrigin, origin, type)
                XCTAssertEqual(restored.inputOrigin.label, origin.label, type)
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
