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
                allowSelfSigned: false)!,
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

    private struct LegacySnapshot: Codable {
        var conversation: Conversation?
        var messages: [Message]
        var lastSequenceId: Int64
        var transcriptGeneration: Int64?
        var syncedAt: Date?
    }

    @MainActor
    func testVersionOneSnapshotRendersReadOnlyUntilAuthoritativeInit() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-legacy-\(UUID().uuidString)")
        XCTAssertTrue(DiskStore.saveVersioned(
            LegacySnapshot(
                conversation: try conversation(),
                messages: [try message(id: "legacy", content: "[]")],
                lastSequenceId: 2,
                transcriptGeneration: 1,
                syncedAt: Date()),
            name: "conv-c1",
            version: 1))

        let api = PhoenixAPI(
            baseURL: URL(string: "https://phoenix.invalid")!, password: nil, allowSelfSigned: false)!
        let credential = Keychain.CredentialRecord(
            password: "", legacyServerURL: api.baseURL.absoluteString)
        let session = ConversationSession(
            conversationId: "c1", api: api, connectivity: ConnectivityMonitor(),
            credentialGeneration: credential.generation,
            legacyPersistenceScope: credential.legacyPersistenceScope(serverURL: api.baseURL))

        XCTAssertEqual(session.messages.map(\.message_id), ["legacy"])
        XCTAssertNil(session.snapshotLoadError)
        XCTAssertFalse(session.acceptsConversationActions)
        let didQueue = await session.send(text: "must not queue")
        XCTAssertFalse(didQueue)

        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 3,
            pendingAnchorSequenceId: 3, pendingEvents: [], pendingTruncated: false)))
        await session.awaitSnapshotPersistenceForTesting()

        XCTAssertTrue(session.acceptsConversationActions)
    }

    @MainActor
    func testVersionOneSnapshotRejectsMissingAndForeignLegacyProvenance() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-rejected-\(UUID().uuidString)")
        XCTAssertTrue(DiskStore.saveVersioned(
            LegacySnapshot(
                conversation: try conversation(), messages: [try message(id: "legacy", content: "[]")],
                lastSequenceId: 2, transcriptGeneration: 1, syncedAt: Date()),
            name: "conv-c1", version: 1))
        let api = PhoenixAPI(
            baseURL: URL(string: "https://phoenix.invalid")!, password: nil, allowSelfSigned: false)!
        let credential = Keychain.CredentialRecord(password: "")
        let scope = ConversationSession.persistenceScope(for: api, credentialGeneration: credential.generation)
        for proof in [nil, "https://other.invalid|\(credential.generation.uuidString)", "\(api.baseURL)|\(UUID())"] as [String?] {
            let session = ConversationSession(
                conversationId: "c1", api: api, connectivity: ConnectivityMonitor(),
                credentialGeneration: credential.generation, legacyPersistenceScope: proof)
            XCTAssertNil(session.conversation)
            XCTAssertTrue(session.messages.isEmpty)
            XCTAssertNotNil(session.snapshotLoadError)
            XCTAssertNotNil(session.lastErrorToast)
            XCTAssertFalse(session.acceptsConversationActions)
            let queued = await session.send(text: "must not queue")
            XCTAssertFalse(queued)
            let saved = await session.flushSnapshotPersistence()
            XCTAssertFalse(saved)
            XCTAssertNil(ConversationSession.cachedConversation(
                conversationId: "c1", persistenceScope: scope, legacyPersistenceScope: proof))
        }
        XCTAssertNotNil(ConversationSession.cachedConversation(
            conversationId: "c1", persistenceScope: scope, legacyPersistenceScope: scope))
        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: "c1"))
    }

    @MainActor
    func testPersistenceScopeUsesOnlyOpaqueGenerationNotPassword() {
        let firstAPI = PhoenixAPI(
            baseURL: URL(string: "https://phoenix.invalid")!, password: "first-secret", allowSelfSigned: false)!
        let secondAPI = PhoenixAPI(
            baseURL: firstAPI.baseURL, password: "second-secret", allowSelfSigned: false)!
        let generation = UUID()
        let scope = ConversationSession.persistenceScope(for: firstAPI, credentialGeneration: generation)
        XCTAssertEqual(scope, "\(firstAPI.baseURL)|\(generation.uuidString)")
        XCTAssertEqual(scope, ConversationSession.persistenceScope(for: secondAPI, credentialGeneration: generation))
        XCTAssertNotEqual(scope, ConversationSession.persistenceScope(for: firstAPI, credentialGeneration: UUID()))
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
            displayData: try json("{\"status\":\"running\",\"tool_starts\":{\"a\":1}}"),
            durationMs: nil,
            transcriptGeneration: 2))
        session.receive(.messageUpdated(
            seq: 2, messageId: "m1", content: nil,
            displayData: try json("{\"status\":\"completed\",\"tool_starts\":{\"b\":2}}"),
            durationMs: 321, transcriptGeneration: 2))
        session.receive(.message(
            seq: 3,
            message: try message(
                id: "m1", content: "[{\"type\":\"text\",\"text\":\"original\"}]")))

        XCTAssertEqual(
            session.messages[0].content.arrayValue?.first?["text"]?.stringValue,
            "patched")
        XCTAssertEqual(session.conversation?.transcript_generation, 2)
        XCTAssertEqual(session.messages[0].display_data?["status"]?.stringValue, "completed")
        XCTAssertEqual(session.messages[0].display_data?["tool_starts"]?["a"]?.numberValue, 1)
        XCTAssertEqual(session.messages[0].display_data?["tool_starts"]?["b"]?.numberValue, 2)
        XCTAssertEqual(session.messages[0].display_data?["duration_ms"]?.numberValue, 321)
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
            displayData: nil, durationMs: nil, transcriptGeneration: nil))

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
    func testHardDeleteClearsLocalTranscriptAndSignalsOwner() async throws {
        var deletedId: String?
        let session = makeSession { deletedId = $0 }
        session.receive(.initSnapshot(.init(
            conversation: try conversation(),
            messages: [try message(id: "m1", content: "[]")],
            agentWorking: false, presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let initialSnapshotSaved = await session.flushSnapshotPersistence()
        XCTAssertTrue(initialSnapshotSaved)
        XCTAssertTrue(ConversationSession.hasCachedSnapshot(conversationId: "c1"))

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "c1"))
        await session.awaitSnapshotPersistenceForTesting()

        XCTAssertTrue(session.isHardDeleted)
        XCTAssertTrue(session.messages.isEmpty)
        XCTAssertNil(session.conversation)
        XCTAssertTrue(session.outbox.entries.isEmpty)
        XCTAssertEqual(deletedId, "c1")
        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: "c1"))
    }

    @MainActor
    func testOfflineAvailabilityRequiresAnAuthoritativeCachedConversation() async throws {
        let session = makeSession()
        session.pauseForBackground()
        let emptySnapshotSaved = await session.flushSnapshotPersistence()
        XCTAssertTrue(emptySnapshotSaved)
        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: "c1"))

        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let authoritativeSnapshotSaved = await session.flushSnapshotPersistence()
        XCTAssertTrue(authoritativeSnapshotSaved)
        XCTAssertTrue(ConversationSession.hasCachedSnapshot(conversationId: "c1"))
    }

    @MainActor
    func testSnapshotRemovalFencesPendingSaves() async throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        session.stop()

        await session.clearCachedSnapshotAndWait()

        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: "c1"))
    }

    @MainActor
    func testCanonicalAuthoritativeMessageReconcilesOptimisticEntry() async throws {
        let session = makeSession()
        let entry = await session.outbox.enqueue(text: "sent once")!

        session.receive(.initSnapshot(.init(
            conversation: try conversation(),
            messages: [try message(
                id: "c1:\(entry.localId)", type: "user",
                content: "{\"text\":\"sent once\"}")],
            agentWorking: true, presentationMode: "working", lastSequenceId: 2,
            pendingAnchorSequenceId: 2, pendingEvents: [], pendingTruncated: false)))
        session.receive(.stateChange(
            seq: 3, state: .string("idle"), presentationMode: "idle",
            stateUpdatedAt: nil))
        let latestSnapshotSaved = await session.flushSnapshotPersistence()

        XCTAssertTrue(latestSnapshotSaved)
        XCTAssertTrue(session.outbox.visibleEntries.isEmpty)
        XCTAssertEqual(session.outbox.entries.first?.status, .reconciled)
    }
}
