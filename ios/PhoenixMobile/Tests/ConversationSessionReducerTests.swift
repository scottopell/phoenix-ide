import XCTest

@testable import PhoenixMobile

final class ConversationSessionReducerTests: XCTestCase {
    @MainActor
    private func makeSession(
        onHardDeleted: @escaping (String) -> Void = { _ in }
    ) -> ConversationSession {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        return ConversationSession(
            conversationId: "c1",
            api: PhoenixAPI(
                baseURL: URL(string: "https://phoenix.invalid")!,
                password: nil,
                allowSelfSigned: false),
            connectivity: ConnectivityMonitor(),
            onHardDeleted: onHardDeleted)
    }

    private func json(_ raw: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(raw.utf8))
    }

    private func conversation(state: String = "{\"type\":\"idle\"}") throws -> Conversation {
        try JSONDecoder().decode(
            Conversation.self,
            from: Data("{\"id\":\"c1\",\"slug\":\"c1\",\"state\":\(state)}".utf8))
    }

    private func message(id: String, type: String = "agent", content: String) throws -> Message {
        try JSONDecoder().decode(
            Message.self,
            from: Data("{\"message_id\":\"\(id)\",\"sequence_id\":2,\"message_type\":\"\(type)\",\"content\":\(content)}".utf8))
    }

    @MainActor
    func testMessageUpdateWaitsForMessageIdentity() throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        session.receive(.messageUpdated(
            seq: 1, messageId: "m1", content: try json("[{\"type\":\"text\",\"text\":\"patched\"}]"),
            displayData: nil, transcriptGeneration: 2))
        session.receive(.message(
            seq: 2,
            message: try message(
                id: "m1", content: "[{\"type\":\"text\",\"text\":\"original\"}]")))

        XCTAssertEqual(
            session.messages[0].content.arrayValue?.first?["text"]?.stringValue,
            "patched")
        XCTAssertEqual(session.conversation?.transcript_generation, 2)
    }

    @MainActor
    func testInitDropsPatchesFromThePreviousStreamBeforeReplay() throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        session.receive(.messageUpdated(
            seq: 1, messageId: "m1",
            content: try json("[{\"type\":\"text\",\"text\":\"stale patch\"}]"),
            displayData: nil, transcriptGeneration: nil))

        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 2,
            pendingAnchorSequenceId: 2, pendingEvents: [], pendingTruncated: false)))
        session.receive(.message(
            seq: 3,
            message: try message(
                id: "m1", content: "[{\"type\":\"text\",\"text\":\"fresh\"}]")))

        XCTAssertEqual(
            session.messages[0].content.arrayValue?.first?["text"]?.stringValue,
            "fresh")
    }

    @MainActor
    func testAgentDoneClearsUntypedWorkingMode() throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(state: "{\"type\":\"provisioning\"}"),
            messages: [], agentWorking: true, presentationMode: "working",
            lastSequenceId: 0, pendingAnchorSequenceId: 0,
            pendingEvents: [], pendingTruncated: false)))

        session.receive(.agentDone(seq: 1))

        XCTAssertEqual(session.typedState, .idle)
        XCTAssertEqual(session.presentationMode, "idle")
        XCTAssertFalse(session.agentWorking)
    }

    @MainActor
    func testHardDeleteClearsLocalTranscriptAndSignalsOwner() throws {
        var deletedId: String?
        let session = makeSession { deletedId = $0 }
        session.receive(.initSnapshot(.init(
            conversation: try conversation(),
            messages: [try message(id: "m1", content: "[]")],
            agentWorking: false, presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "c1"))

        XCTAssertTrue(session.isHardDeleted)
        XCTAssertTrue(session.messages.isEmpty)
        XCTAssertNil(session.conversation)
        XCTAssertTrue(session.outbox.entries.isEmpty)
        XCTAssertEqual(deletedId, "c1")
    }

    @MainActor
    func testCanonicalAuthoritativeMessageReconcilesOptimisticEntry() throws {
        let session = makeSession()
        let entry = session.outbox.enqueue(text: "sent once")!

        session.receive(.initSnapshot(.init(
            conversation: try conversation(),
            messages: [try message(
                id: "c1:\(entry.localId)", type: "user",
                content: "{\"text\":\"sent once\"}")],
            agentWorking: true, presentationMode: "working", lastSequenceId: 2,
            pendingAnchorSequenceId: 2, pendingEvents: [], pendingTruncated: false)))

        XCTAssertTrue(session.outbox.visibleEntries.isEmpty)
    }
}
