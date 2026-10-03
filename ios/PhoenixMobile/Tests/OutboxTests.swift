import XCTest

@testable import PhoenixMobile

// Contract tests for the offline message queue. The contract is
// specs/user_message_queue/user_message_queue.allium (plus the iOS
// deviations recorded in specs/ios_client REQ-IOS-002); tests are named
// after the spec rules they pin down. DiskStore is pointed at a fresh
// temp directory per test, so persistence behaviour — the load-bearing
// part — is exercised for real.
final class OutboxTests: XCTestCase {

    /// Point DiskStore at a fresh temp dir. Returns it for direct fixture
    /// writes (e.g. pre-seeding entries "from a previous launch").
    @MainActor
    @discardableResult
    private func freshDiskStore() -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-outbox-tests-\(UUID().uuidString)")
        DiskStore.baseDirectory = dir
        return dir
    }

    @MainActor
    private func makeEntry(
        conversationId: String,
        status: OutboxEntry.Status = .pending,
        acceptedByServer: Bool = false,
        createdAt: Date = Date(),
        acceptedAt: Date? = nil
    ) -> OutboxEntry {
        OutboxEntry(
            localId: UUID().uuidString.lowercased(),
            conversationId: conversationId,
            text: "hello",
            images: [],
            status: status,
            acceptedByServer: acceptedByServer,
            createdAt: createdAt,
            acceptedAt: acceptedAt,
            lastError: nil,
            attemptCount: 0)
    }

    // MARK: - EnqueueLocalMessage

    @MainActor
    func testEnqueueCreatesPendingVisibleEntry() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "hi")!
        XCTAssertEqual(entry.status, .pending)
        XCTAssertFalse(entry.acceptedByServer)
        XCTAssertTrue(entry.isVisible)
        XCTAssertEqual(outbox.visibleEntries.map(\.localId), [entry.localId])
    }

    @MainActor
    func testEnqueuePersistsBeforeAnySendAttempt() async {
        // The entry must survive a "restart" (new instance) even though no
        // POST was ever attempted — enqueue itself is the durability point.
        freshDiskStore()
        let entry = await Outbox(conversationId: "c1").enqueue(text: "queued in a tunnel")!
        let rehydrated = Outbox(conversationId: "c1")
        XCTAssertEqual(rehydrated.visibleEntries.map(\.localId), [entry.localId])
        XCTAssertEqual(rehydrated.visibleEntries[0].text, "queued in a tunnel")
        XCTAssertEqual(rehydrated.visibleEntries[0].status, .pending)
    }

    @MainActor
    func testArchiveGuardSeesPersistedVisibleEntries() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "do not lose me")!
        XCTAssertEqual(Outbox.storedContents(conversationId: "c1"), .hasVisibleEntries)

        await outbox.dismiss(entry.localId)
        XCTAssertEqual(Outbox.storedContents(conversationId: "c1"), .empty)
    }

    @MainActor
    func testDeliveryRemainsBlockedUntilFailedEnqueueWriteRecovers() async throws {
        let blockedRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-outbox-blocked-\(UUID().uuidString)")
        try Data("not a directory".utf8).write(to: blockedRoot)
        DiskStore.baseDirectory = blockedRoot

        let outbox = Outbox(conversationId: "c1")
        let queuedEntry = await outbox.enqueue(text: "must be durable")
        let entry = try XCTUnwrap(queuedEntry)
        XCTAssertFalse(outbox.persistenceHealthy)
        let firstPreparation = await outbox.prepareForDelivery()
        XCTAssertFalse(firstPreparation, "POST must stay blocked without a disk copy")

        let secondPreparation = await outbox.prepareForDelivery()
        XCTAssertFalse(secondPreparation)
        XCTAssertEqual(outbox.entries.map(\.localId), [entry.localId])
        try FileManager.default.removeItem(at: blockedRoot)
        let recoveredPreparation = await outbox.prepareForDelivery()
        XCTAssertTrue(recoveredPreparation)
        XCTAssertEqual(
            Outbox(conversationId: "c1").visibleEntries.map(\.localId),
            [entry.localId])
    }

    @MainActor
    func testArchiveGuardTreatsNewerOutboxAsInaccessible() async throws {
        let root = freshDiskStore()
        let directory = root.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Data(#"{"schema_version":999,"payload":[]}"#.utf8).write(
            to: directory.appendingPathComponent("outbox-c1.json"))

        XCTAssertEqual(Outbox.storedContents(conversationId: "c1"), .inaccessible)
        XCTAssertFalse(Outbox(conversationId: "c1").persistenceHealthy)
    }
    // MARK: - PostAccepted{AsSteeringQueued, AsPendingReflection}

    @MainActor
    func testAcceptedSteeringBecomesSteeringQueued() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "hi")!
        outbox.markAccepted(entry.localId, steering: true)
        XCTAssertEqual(outbox.entries[0].status, .steeringQueued)
        XCTAssertTrue(outbox.entries[0].acceptedByServer)
    }

    @MainActor
    func testAcceptedNonSteeringStaysPendingUntilReflected() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "hi")!
        outbox.markAccepted(entry.localId, steering: false)
        XCTAssertEqual(outbox.entries[0].status, .pending)
        XCTAssertTrue(outbox.entries[0].acceptedByServer)
        XCTAssertTrue(outbox.entries[0].isVisible, "accepted-but-unreflected must keep rendering")
    }

    // MARK: - PostFailedIsRetryable / RetryFailedMessage / DismissLocalMessage

    @MainActor
    func testServerRejectionMarksFailedWithError() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "hi")!
        outbox.markFailed(entry.localId, error: "HTTP 400: bad request")
        XCTAssertEqual(outbox.entries[0].status, .failed)
        XCTAssertEqual(outbox.entries[0].lastError, "HTTP 400: bad request")
        XCTAssertTrue(outbox.entries[0].isVisible)
    }

    @MainActor
    func testRetryReturnsFailedEntryToPending() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "hi")!
        outbox.markFailed(entry.localId, error: "boom")
        outbox.retry(entry.localId)
        XCTAssertEqual(outbox.entries[0].status, .pending)
        XCTAssertNil(outbox.entries[0].lastError)
    }

    @MainActor
    func testRetryIsGuardedToRetryableStates() async {
        // Spec precondition: retry applies to failed / recoverable only.
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "hi")!
        outbox.markAccepted(entry.localId, steering: true)
        outbox.retry(entry.localId)
        XCTAssertEqual(outbox.entries[0].status, .steeringQueued, "retry must not touch steering_queued")
    }

    @MainActor
    func testDismissHidesEntryAndPrunesOnRestart() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "hi")!
        outbox.markFailed(entry.localId, error: "boom")
        await outbox.dismiss(entry.localId)
        XCTAssertTrue(outbox.visibleEntries.isEmpty)
        // Terminal entries carry no future obligation — gone after restart.
        XCTAssertTrue(Outbox(conversationId: "c1").entries.isEmpty)
    }

    // MARK: - AuthoritativeMessageReconcilesQueueEntry

    @MainActor
    func testReconcileHidesEntriesPresentInServerHistory() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let a = await outbox.enqueue(text: "first")!
        let b = await outbox.enqueue(text: "second")!
        outbox.reconcile(authoritativeMessageIds: [a.localId])
        XCTAssertEqual(outbox.visibleEntries.map(\.localId), [b.localId])
    }

    @MainActor
    func testAuthoritativeHistorySuppressesDuplicateBeforeDurableReconciliation() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "first")!
        outbox.suppress(authoritativeMessageIds: [entry.localId])

        XCTAssertTrue(outbox.visibleEntries.isEmpty)
        XCTAssertEqual(outbox.entries[0].status, .pending)
        XCTAssertEqual(Outbox(conversationId: "c1").visibleEntries.map(\.localId), [entry.localId])
    }

    @MainActor
    func testCanonicalAuthoritativeIdentitySuppressesDuplicate() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "first")!

        outbox.suppress(authoritativeMessageIds: ["c1:\(entry.localId)"])

        XCTAssertTrue(outbox.visibleEntries.isEmpty)
        XCTAssertEqual(outbox.entries[0].status, .pending)
    }

    @MainActor
    func testReconcileRecognizesConversationScopedCanonicalMessageId() async {
        freshDiskStore()
        let entry = makeEntry(
            conversationId: "c1", status: .recoverableInconsistency,
            acceptedByServer: true)
        DiskStore.saveVersioned(
            [entry], name: "outbox-c1", version: Outbox.schemaVersion)
        let outbox = Outbox(conversationId: "c1")

        outbox.reconcile(authoritativeMessageIds: ["c1:\(entry.localId)"])

        XCTAssertTrue(outbox.visibleEntries.isEmpty)
    }

    @MainActor
    func testCanonicalMessageIdFromAnotherConversationDoesNotReconcile() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "scoped identity")!

        outbox.reconcile(authoritativeMessageIds: ["c2:\(entry.localId)"])

        XCTAssertEqual(outbox.visibleEntries.map(\.localId), [entry.localId])
    }

    @MainActor
    func testReconcileAppliesToSteeringQueuedEntries() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "steer me")!
        outbox.markAccepted(entry.localId, steering: true)
        outbox.reconcile(authoritativeMessageIds: [entry.localId])
        XCTAssertTrue(outbox.visibleEntries.isEmpty)
    }

    @MainActor
    func testReconcileAppliesToRehydratedEntriesAfterRestart() async {
        // Identity join must hold across app restarts: the persisted
        // localId is what matches the server's message_id.
        freshDiskStore()
        let entry = await Outbox(conversationId: "c1").enqueue(text: "hi")!
        let rehydrated = Outbox(conversationId: "c1")
        rehydrated.reconcile(authoritativeMessageIds: [entry.localId])
        XCTAssertTrue(rehydrated.visibleEntries.isEmpty)
    }

    // MARK: - RehydrateQueueForConversationOnly

    @MainActor
    func testForeignConversationEntriesAreDroppedOnRehydration() async {
        // An entry tagged with another conversation can never reconcile
        // here — it must be unrenderable, not a phantom pending bubble.
        freshDiskStore()
        let foreign = makeEntry(conversationId: "other")
        let mine = makeEntry(conversationId: "c1")
        DiskStore.save([foreign, mine], name: "outbox-c1")
        let outbox = Outbox(conversationId: "c1")
        XCTAssertEqual(outbox.entries.map(\.localId), [mine.localId])
    }

    @MainActor
    func testQueuesAreScopedPerConversation() async {
        freshDiskStore()
        let a = Outbox(conversationId: "a")
        let b = Outbox(conversationId: "b")
        _ = await a.enqueue(text: "for a")
        XCTAssertTrue(b.entries.isEmpty)
        XCTAssertTrue(Outbox(conversationId: "b").entries.isEmpty)
    }

    // MARK: - AcceptedButCausallyProvenMissingBecomesRecoverable (time approximation)

    @MainActor
    func testStaleAcceptedPendingEntrySurfacesAsRecoverable() async {
        freshDiskStore()
        let stale = makeEntry(
            conversationId: "c1", status: .pending, acceptedByServer: true,
            createdAt: Date().addingTimeInterval(-300),
            acceptedAt: Date().addingTimeInterval(-120))
        DiskStore.save([stale], name: "outbox-c1")
        let outbox = Outbox(conversationId: "c1")
        outbox.surfaceStaleAcceptedEntries(window: 60)
        XCTAssertEqual(outbox.entries[0].status, .recoverableInconsistency)
    }

    @MainActor
    func testStalenessWindowRunsFromAcceptanceNotComposition() async {
        // A message composed offline long ago but accepted just now must
        // get the full window before being flagged — otherwise every
        // subway-composed message briefly shows a false alarm on arrival.
        freshDiskStore()
        let justAccepted = makeEntry(
            conversationId: "c1", status: .pending, acceptedByServer: true,
            createdAt: Date().addingTimeInterval(-3600),
            acceptedAt: Date())
        DiskStore.save([justAccepted], name: "outbox-c1")
        let outbox = Outbox(conversationId: "c1")
        outbox.surfaceStaleAcceptedEntries(window: 60)
        XCTAssertEqual(outbox.entries[0].status, .pending)
    }

    @MainActor
    func testSteeringQueuedIsExemptFromStalenessSurfacing() async {
        // A steering-queued entry legitimately waits out the current turn.
        freshDiskStore()
        let steering = makeEntry(
            conversationId: "c1", status: .steeringQueued, acceptedByServer: true,
            createdAt: Date().addingTimeInterval(-3600))
        DiskStore.save([steering], name: "outbox-c1")
        let outbox = Outbox(conversationId: "c1")
        outbox.surfaceStaleAcceptedEntries(window: 60)
        XCTAssertEqual(outbox.entries[0].status, .steeringQueued)
    }

    @MainActor
    func testUnacceptedPendingEntryIsExemptFromStalenessSurfacing() async {
        // Never-accepted entries are simply waiting for connectivity;
        // they must stay quietly pending, not alarm the user.
        freshDiskStore()
        let offline = makeEntry(
            conversationId: "c1", status: .pending, acceptedByServer: false,
            createdAt: Date().addingTimeInterval(-3600))
        DiskStore.save([offline], name: "outbox-c1")
        let outbox = Outbox(conversationId: "c1")
        outbox.surfaceStaleAcceptedEntries(window: 60)
        XCTAssertEqual(outbox.entries[0].status, .pending)
    }

    @MainActor
    func testDismissedEntryIsNotResurrectedByLateAcceptOrFailure() async {
        // A replayed steer_message_queued event or a POST that completes
        // after the user discarded the entry must not bring it back.
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "hi")!
        outbox.markFailed(entry.localId, error: "boom")
        await outbox.dismiss(entry.localId)
        outbox.markAccepted(entry.localId, steering: true)
        XCTAssertEqual(outbox.entries[0].status, .dismissed)
        outbox.markFailed(entry.localId, error: "late failure")
        XCTAssertEqual(outbox.entries[0].status, .dismissed)
        XCTAssertTrue(outbox.visibleEntries.isEmpty)
    }

    @MainActor
    func testClearAndWaitFencesQueuedWritesBeforeDirectoryRemoval() async {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = await outbox.enqueue(text: "must stay deleted")!
        outbox.markFailed(entry.localId, error: "queued write")

        await outbox.clearAndWait()
        DiskStore.removeAll()

        XCTAssertTrue(Outbox(conversationId: "c1").entries.isEmpty)
    }

    @MainActor
    func testRecoverableEntryCanBeRetriedOrDismissed() async {
        freshDiskStore()
        let stale = makeEntry(
            conversationId: "c1", status: .recoverableInconsistency, acceptedByServer: true,
            createdAt: Date().addingTimeInterval(-120))
        DiskStore.save([stale], name: "outbox-c1")

        let outbox = Outbox(conversationId: "c1")
        outbox.retry(stale.localId)
        XCTAssertEqual(outbox.entries[0].status, .pending)
        // Regression pin: without clearing acceptedByServer, the drain loop
        // (which skips accepted entries) would never re-POST this entry and
        // the staleness check would bounce it straight back to recoverable.
        XCTAssertFalse(outbox.entries[0].acceptedByServer)
    }
}
