import XCTest

@testable import PhoenixMobile

// Contract tests for the SSE frame parser — the testing pattern for this
// app (see ios/README.md "Testing"): pure components get contract tests,
// one test per rule of the contract they implement; views stay untested.
// The contract here is the SSE wire format as the Phoenix server emits it
// (named events, JSON data lines, comment keep-alives, blank-line frame
// boundaries).
final class SSEParserTests: XCTestCase {

    private func message(id: String, sequence: Int64, text: String) -> Message {
        Message(
            message_id: id,
            conversation_id: "c1",
            sequence_id: sequence,
            message_type: "user",
            content: .string(text),
            display_data: nil,
            created_at: nil)
    }

    /// Feed a raw stream through the byte-level parser, collecting frames.
    private func frames(from raw: String) -> [SSEFrame] {
        var parser = SSEParser()
        var out: [SSEFrame] = []
        for byte in Array(raw.utf8) {
            if let frame = parser.consume(byte) {
                out.append(frame)
            }
        }
        return out
    }

    func testNamedEventWithDataDispatchesOnBlankLine() {
        let out = frames(from: "event: message\ndata: {\"a\":1}\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].event, "message")
        XCTAssertEqual(out[0].data, "{\"a\":1}")
    }

    func testNoFrameEmittedBeforeBlankLine() {
        let out = frames(from: "event: message\ndata: {}\n")
        XCTAssertTrue(out.isEmpty, "frame must not dispatch until the blank line")
    }

    func testEventNameDefaultsToMessage() {
        let out = frames(from: "data: hello\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].event, "message")
        XCTAssertEqual(out[0].data, "hello")
    }

    func testMultiLineDataJoinedWithNewline() {
        let out = frames(from: "event: token\ndata: line1\ndata: line2\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].data, "line1\nline2")
    }

    func testCommentKeepAlivesProduceNoFrames() {
        // The server sends `: keep-alive` comments during quiet periods;
        // they must neither surface as frames nor break a following frame.
        let out = frames(from: ": keep-alive\n\n: ping\nevent: agent_done\ndata: {}\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].event, "agent_done")
    }

    func testCarriageReturnsStripped() {
        let out = frames(from: "event: message\r\ndata: x\r\n\r\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].event, "message")
        XCTAssertEqual(out[0].data, "x")
    }

    func testOnlyOneLeadingSpaceStrippedFromValue() {
        // Spec: a single space after the colon is part of the delimiter;
        // further whitespace belongs to the value.
        let out = frames(from: "data:  two spaces\n\n")
        XCTAssertEqual(out[0].data, " two spaces")
    }

    func testValueMayContainColons() {
        let out = frames(from: "data: {\"url\":\"https://x\"}\n\n")
        XCTAssertEqual(out[0].data, "{\"url\":\"https://x\"}")
    }

    func testNoSpaceAfterColonAccepted() {
        let out = frames(from: "data:tight\n\n")
        XCTAssertEqual(out[0].data, "tight")
    }

    func testBackToBackFrames() {
        let out = frames(from: "event: a\ndata: 1\n\nevent: b\ndata: 2\n\n")
        XCTAssertEqual(out.map(\.event), ["a", "b"])
        XCTAssertEqual(out.map(\.data), ["1", "2"])
    }

    func testBlankLinesWithoutAccumulatedFieldsEmitNothing() {
        let out = frames(from: "\n\n\n")
        XCTAssertTrue(out.isEmpty)
    }

    func testEventNameResetsBetweenFrames() {
        // A frame with only a name set (no data) still dispatches, and the
        // name must not leak into the next frame.
        let out = frames(from: "event: named\n\ndata: x\n\n")
        XCTAssertEqual(out.count, 2)
        XCTAssertEqual(out[0].event, "named")
        XCTAssertEqual(out[0].data, "")
        XCTAssertEqual(out[1].event, "message")
        XCTAssertEqual(out[1].data, "x")
    }

    func testUnknownFieldsIgnored() {
        let out = frames(from: "id: 42\nretry: 1000\nevent: message\ndata: x\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].data, "x")
    }
    func testInitDecodesTranscriptGenerationAndTailCoverage() {
        let json = """
        {
          "conversation": {"id":"c1","slug":"one","transcript_generation":7},
          "messages": [],
          "agent_working": false,
          "last_sequence_id": 12,
          "pending_anchor_sequence_id": 12,
          "pending_events": [],
          "pending_truncated": false,
          "transcript_generation": 7,
          "transcript_coverage": "tail"
        }
        """
        guard case .initSnapshot(let snapshot) = PhoenixEvent.decode(
            frame: SSEFrame(event: "init", data: json))
        else {
            return XCTFail("expected a decoded init snapshot")
        }
        XCTAssertEqual(snapshot.transcriptGeneration, 7)
        XCTAssertEqual(snapshot.transcriptCoverage, .tail)
    }

    func testTailInitMergesWithoutDroppingOlderCachedMessages() {
        let older = message(id: "old", sequence: 1, text: "older")
        let staleOverlap = message(id: "same", sequence: 2, text: "stale")
        let freshOverlap = message(id: "same", sequence: 2, text: "fresh")
        let latest = message(id: "new", sequence: 3, text: "latest")

        let merged = ConversationSession.reconcileTranscript(
            existing: [older, staleOverlap],
            incoming: [freshOverlap, latest],
            coverage: .tail,
            generationMatches: true)

        XCTAssertEqual(merged.map(\.message_id), ["old", "same", "new"])
        XCTAssertEqual(merged[1].content, .string("fresh"))
    }

    func testGenerationChangeReplacesCachedTranscriptEvenForTailCoverage() {
        let merged = ConversationSession.reconcileTranscript(
            existing: [message(id: "stale", sequence: 1, text: "stale")],
            incoming: [message(id: "fresh", sequence: 5, text: "fresh")],
            coverage: .tail,
            generationMatches: false)

        XCTAssertEqual(merged.map(\.message_id), ["fresh"])
    }

    func testDiskRestoreReplaysFromPendingAnchor() {
        XCTAssertEqual(
            ConversationSession.replayFloor(
                previous: 50,
                anchor: 40,
                serverTip: 60,
                generationMatches: true,
                restoredFromDisk: true),
            40)
        XCTAssertEqual(
            ConversationSession.replayFloor(
                previous: 50,
                anchor: 40,
                serverTip: 60,
                generationMatches: true,
                restoredFromDisk: false),
            50)
    }

    func testServerSequenceRegressionResetsReplayFloorToNewAnchor() {
        XCTAssertEqual(
            ConversationSession.replayFloor(
                previous: 50,
                anchor: 4,
                serverTip: 10,
                generationMatches: true,
                restoredFromDisk: false),
            4)
    }

    func testChatDecodingFailureRemainsRetryable() {
        let error = APIError.decoding(underlying: URLError(.cannotDecodeContentData))
        XCTAssertTrue(error.isRetryableChatDeliveryFailure)
        XCTAssertFalse(
            APIError.http(status: 400, body: "rejected").isRetryableChatDeliveryFailure)
        XCTAssertTrue(APIError.http(status: 404, body: "gone").isNotFound)
        XCTAssertFalse(APIError.http(status: 500, body: "retry").isNotFound)
        XCTAssertTrue(
            APIError.http(status: 401, body: "unauthorized")
                .isPermanentStreamAuthenticationFailure)
        XCTAssertTrue(
            APIError.http(status: 403, body: "forbidden")
                .isPermanentStreamAuthenticationFailure)
        XCTAssertFalse(
            APIError.http(status: 500, body: "retry")
                .isPermanentStreamAuthenticationFailure)
    }

    func testStateChangeRetainsServerTimestamp() {
        let json = """
        {
          "sequence_id": 9,
          "state": {"type": "idle"},
          "presentation_mode": "idle",
          "state_updated_at": "2026-08-08T12:34:56Z"
        }
        """
        guard case .stateChange(_, _, _, let stateUpdatedAt) = PhoenixEvent.decode(
            frame: SSEFrame(event: "state_change", data: json))
        else { return XCTFail("expected a decoded state change") }
        XCTAssertEqual(stateUpdatedAt, "2026-08-08T12:34:56Z")
    }

    func testSnapshotMessagesStopAtProvenDurableAnchor() {
        let persisted = message(id: "persisted", sequence: 4, text: "saved")
        let eager = message(id: "eager", sequence: 5, text: "not saved yet")
        XCTAssertEqual(
            ConversationSession.durableMessages([persisted, eager], through: 4),
            [persisted])
    }

    func testInitDurableCeilingIncludesMessagesReadAfterTheRingSnapshot() {
        let committedAfterAnchor = message(id: "committed", sequence: 6, text: "saved")
        XCTAssertEqual(
            ConversationSession.durableCeilingAfterInit(
                anchor: 4, messages: [committedAfterAnchor]),
            6)
    }

    func testCommittedLiveMessagesAdvanceDurableBoundaryButEagerAgentDoesNot() {
        let toolResult = Message(
            message_id: "tool", conversation_id: "c1", sequence_id: 6,
            message_type: "tool", content: .string("done"),
            display_data: nil, created_at: nil)
        let eagerAgent = Message(
            message_id: "agent", conversation_id: "c1", sequence_id: 7,
            message_type: "agent", content: .string("working"),
            display_data: nil, created_at: nil)

        XCTAssertEqual(
            ConversationSession.durableCeilingAfterLiveMessage(
                current: 4, message: toolResult),
            6)
        XCTAssertEqual(
            ConversationSession.durableCeilingAfterLiveMessage(
                current: 6, message: eagerAgent),
            6)
    }

    func testPreserveCoverageKeepsGenerationMatchedTranscript() {
        let cached = message(id: "cached", sequence: 1, text: "cached")
        let merged = ConversationSession.reconcileTranscript(
            existing: [cached],
            incoming: [],
            coverage: .preserve,
            generationMatches: true)
        XCTAssertEqual(merged, [cached])
    }

    func testConversationDisplayTitlePrefersServerTitleBeforeDirectory() throws {
        let data = Data(
            "{\"id\":\"c1\",\"slug\":\"one\",\"title\":\"Readable title\",\"cwd\":\"/tmp/repo\"}"
                .utf8)
        let conversation = try JSONDecoder().decode(Conversation.self, from: data)
        XCTAssertEqual(conversation.displayTitle, "Readable title")
    }

    func testChatResponseRetainsAlreadyPersistedSignal() throws {
        let response = try JSONDecoder().decode(
            ChatResponse.self,
            from: Data("{\"queued\":false,\"already_persisted\":true}".utf8))
        XCTAssertEqual(response.already_persisted, true)
    }

    func testMessageUpdateRetainsToolDuration() {
        let json = """
        {
          "sequence_id": 9,
          "message_id": "tool-result",
          "duration_ms": 1234
        }
        """
        guard case .messageUpdated(
            let sequence, let messageId, _, _, let durationMs, _
        ) = PhoenixEvent.decode(frame: SSEFrame(event: "message_updated", data: json)) else {
            return XCTFail("expected a decoded message update")
        }
        XCTAssertEqual(sequence, 9)
        XCTAssertEqual(messageId, "tool-result")
        XCTAssertEqual(durationMs, 1234)
    }
    func testHardDeletionRetainsConversationIdentity() {
        let json = #"{"sequence_id":11,"conversation_id":"c1"}"#
        guard case .conversationHardDeleted(let sequence, let conversationId) =
            PhoenixEvent.decode(frame: SSEFrame(event: "conversation_hard_deleted", data: json))
        else {
            return XCTFail("expected a decoded hard-deletion event")
        }
        XCTAssertEqual(sequence, 11)
        XCTAssertEqual(conversationId, "c1")
    }

    @MainActor
    func testNetworkLossImmediatelyMovesAnOpenSessionOffline() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-connectivity-tests-\(UUID().uuidString)")
        let connectivity = ConnectivityMonitor()
        let session = ConversationSession(
            conversationId: "c1",
            api: PhoenixAPI(
                baseURL: URL(string: "https://phoenix.invalid")!,
                password: nil,
                allowSelfSigned: false)!,
            connectivity: connectivity)

        session.start()
        connectivity.setOnlineForTesting(false)

        XCTAssertEqual(session.connection, .offline)
        session.stop()
    }

    func testRetryableErrorSignalIsDecoded() {
        let frame = SSEFrame(
            event: "error",
            data: #"{"sequence_id":9,"error":{"kind":"retryable","message":"approval failed"}}"#)

        guard case .errorEvent(let seq, let message, let retryable) = PhoenixEvent.decode(
            frame: frame)
        else { return XCTFail("expected retryable error event") }
        XCTAssertEqual(seq, 9)
        XCTAssertEqual(message, "approval failed")
        XCTAssertTrue(retryable)
    }

    func testHardDeleteEventCarriesConversationIdentity() {
        let frame = SSEFrame(
            event: "conversation_hard_deleted",
            data: "{\"type\":\"conversation_hard_deleted\",\"sequence_id\":42,\"conversation_id\":\"c1\"}")
        guard case .conversationHardDeleted(let seq, let conversationId) =
            PhoenixEvent.decode(frame: frame)
        else {
            return XCTFail("expected typed hard-delete event")
        }
        XCTAssertEqual(seq, 42)
        XCTAssertEqual(conversationId, "c1")
    }
}
