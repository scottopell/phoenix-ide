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
    func testEnqueueCreatesPendingVisibleEntry() {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "hi")!
        XCTAssertEqual(entry.status, .pending)
        XCTAssertFalse(entry.acceptedByServer)
        XCTAssertTrue(entry.isVisible)
        XCTAssertEqual(outbox.visibleEntries.map(\.localId), [entry.localId])
    }

    @MainActor
    func testEnqueuePersistsBeforeAnySendAttempt() {
        // The entry must survive a "restart" (new instance) even though no
        // POST was ever attempted — enqueue itself is the durability point.
        freshDiskStore()
        let entry = Outbox(conversationId: "c1").enqueue(text: "queued in a tunnel")!
        let rehydrated = Outbox(conversationId: "c1")
        XCTAssertEqual(rehydrated.visibleEntries.map(\.localId), [entry.localId])
        XCTAssertEqual(rehydrated.visibleEntries[0].text, "queued in a tunnel")
        XCTAssertEqual(rehydrated.visibleEntries[0].status, .pending)
    }

    @MainActor
    func testDeliveryIsBlockedUntilQueueCanBePersisted() throws {
        let file = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-unwritable-store-\(UUID().uuidString)")
        try Data("not a directory".utf8).write(to: file)
        DiskStore.baseDirectory = file

        let outbox = Outbox(conversationId: "c1")
        _ = outbox.enqueue(text: "must stay local")

        XCTAssertFalse(outbox.prepareForDelivery())
        XCTAssertTrue(outbox.entries.isEmpty)
    }

    // MARK: - PostAccepted{AsSteeringQueued, AsPendingReflection}

    @MainActor
    func testAcceptedSteeringBecomesSteeringQueued() {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "hi")!
        outbox.markAccepted(entry.localId, steering: true)
        XCTAssertEqual(outbox.entries[0].status, .steeringQueued)
        XCTAssertTrue(outbox.entries[0].acceptedByServer)
    }

    @MainActor
    func testAcceptedNonSteeringStaysPendingUntilReflected() {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "hi")!
        outbox.markAccepted(entry.localId, steering: false)
        XCTAssertEqual(outbox.entries[0].status, .pending)
        XCTAssertTrue(outbox.entries[0].acceptedByServer)
        XCTAssertTrue(outbox.entries[0].isVisible, "accepted-but-unreflected must keep rendering")
    }

    // MARK: - PostFailedIsRetryable / RetryFailedMessage / DismissLocalMessage

    @MainActor
    func testServerRejectionMarksFailedWithError() {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "hi")!
        outbox.markFailed(entry.localId, error: "HTTP 400: bad request")
        XCTAssertEqual(outbox.entries[0].status, .failed)
        XCTAssertEqual(outbox.entries[0].lastError, "HTTP 400: bad request")
        XCTAssertTrue(outbox.entries[0].isVisible)
    }

    @MainActor
    func testRetryReturnsFailedEntryToPending() {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "hi")!
        outbox.markFailed(entry.localId, error: "boom")
        outbox.retry(entry.localId)
        XCTAssertEqual(outbox.entries[0].status, .pending)
        XCTAssertNil(outbox.entries[0].lastError)
    }

    @MainActor
    func testRetryIsGuardedToRetryableStates() {
        // Spec precondition: retry applies to failed / recoverable only.
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "hi")!
        outbox.markAccepted(entry.localId, steering: true)
        outbox.retry(entry.localId)
        XCTAssertEqual(outbox.entries[0].status, .steeringQueued, "retry must not touch steering_queued")
    }

    @MainActor
    func testDismissHidesEntryAndPrunesOnRestart() {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "hi")!
        outbox.markFailed(entry.localId, error: "boom")
        outbox.dismiss(entry.localId)
        XCTAssertTrue(outbox.visibleEntries.isEmpty)
        // Terminal entries carry no future obligation — gone after restart.
        XCTAssertTrue(Outbox(conversationId: "c1").entries.isEmpty)
    }

    // MARK: - AuthoritativeMessageReconcilesQueueEntry

    @MainActor
    func testReconcileHidesEntriesPresentInServerHistory() {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let a = outbox.enqueue(text: "first")!
        let b = outbox.enqueue(text: "second")!
        outbox.reconcile(authoritativeMessageIds: [a.localId])
        XCTAssertEqual(outbox.visibleEntries.map(\.localId), [b.localId])
    }

    @MainActor
    func testReconcileAppliesToSteeringQueuedEntries() {
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "steer me")!
        outbox.markAccepted(entry.localId, steering: true)
        outbox.reconcile(authoritativeMessageIds: [entry.localId])
        XCTAssertTrue(outbox.visibleEntries.isEmpty)
    }

    @MainActor
    func testReconcileAppliesToRehydratedEntriesAfterRestart() {
        // Identity join must hold across app restarts: the persisted
        // localId is what matches the server's message_id.
        freshDiskStore()
        let entry = Outbox(conversationId: "c1").enqueue(text: "hi")!
        let rehydrated = Outbox(conversationId: "c1")
        rehydrated.reconcile(authoritativeMessageIds: [entry.localId])
        XCTAssertTrue(rehydrated.visibleEntries.isEmpty)
    }

    // MARK: - RehydrateQueueForConversationOnly

    @MainActor
    func testForeignConversationEntriesAreDroppedOnRehydration() {
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
    func testQueuesAreScopedPerConversation() {
        freshDiskStore()
        let a = Outbox(conversationId: "a")
        let b = Outbox(conversationId: "b")
        _ = a.enqueue(text: "for a")
        XCTAssertTrue(b.entries.isEmpty)
        XCTAssertTrue(Outbox(conversationId: "b").entries.isEmpty)
    }

    // MARK: - AcceptedButCausallyProvenMissingBecomesRecoverable (time approximation)

    @MainActor
    func testStaleAcceptedPendingEntrySurfacesAsRecoverable() {
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
    func testStalenessWindowRunsFromAcceptanceNotComposition() {
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
    func testSteeringQueuedIsExemptFromStalenessSurfacing() {
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
    func testUnacceptedPendingEntryIsExemptFromStalenessSurfacing() {
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
    func testDismissedEntryIsNotResurrectedByLateAcceptOrFailure() {
        // A replayed steer_message_queued event or a POST that completes
        // after the user discarded the entry must not bring it back.
        freshDiskStore()
        let outbox = Outbox(conversationId: "c1")
        let entry = outbox.enqueue(text: "hi")!
        outbox.markFailed(entry.localId, error: "boom")
        outbox.dismiss(entry.localId)
        outbox.markAccepted(entry.localId, steering: true)
        XCTAssertEqual(outbox.entries[0].status, .dismissed)
        outbox.markFailed(entry.localId, error: "late failure")
        XCTAssertEqual(outbox.entries[0].status, .dismissed)
        XCTAssertTrue(outbox.visibleEntries.isEmpty)
    }

    @MainActor
    func testRecoverableEntryCanBeRetriedOrDismissed() {
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
