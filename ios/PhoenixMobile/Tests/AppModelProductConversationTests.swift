import XCTest

@testable import PhoenixMobile

@MainActor
final class AppModelProductConversationTests: XCTestCase {
    private func conversation(
        id: String,
        aggregateId: String? = nil,
        slug: String? = nil,
        title: String? = nil,
        taskTitle: String? = nil,
        archived: Bool? = nil,
        mode: String? = nil,
        updatedAt: String? = nil,
        runtimeRole: String? = nil
    ) -> Conversation {
        Conversation(
            id: id,
            product_conversation_id: aggregateId,
            slug: slug,
            title: title,
            model: nil,
            cwd: nil,
            created_at: nil,
            updated_at: updatedAt,
            message_count: nil,
            state: nil,
            state_updated_at: nil,
            branch_name: nil,
            task_title: taskTitle,
            archived: archived,
            project_name: nil,
            conv_mode_label: nil,
            presentation_mode: mode,
            requires_action: nil,
            transcript_generation: nil,
            runtime_role: runtimeRole)
    }

    private func persistReadableSnapshot(conversation: Conversation) {
        struct Snapshot: Codable {
            var conversation: Conversation?
            var messages: [Message]
            var lastSequenceId: Int64
            var transcriptGeneration: Int64?
            var syncedAt: Date?
        }

        DiskStore.saveVersioned(
            Snapshot(
                conversation: conversation,
                messages: [],
                lastSequenceId: 0,
                transcriptGeneration: 1,
                syncedAt: Date()),
            name: "conv-\(conversation.id)",
            version: 1)
    }

    private func message(_ id: String, sequence: Int64) -> Message {
        Message(
            message_id: id,
            conversation_id: nil,
            sequence_id: sequence,
            message_type: "user",
            content: .object(["text": .string(id)]),
            display_data: nil,
            created_at: nil)
    }

    private func closeSnapshot(phase: ProductConversationClosePhase) -> ProductConversationClose {
        ProductConversationClose(
            attempt_id: "attempt",
            phase: phase,
            confirmation_snapshot: nil,
            inspections: [],
            losses: [],
            residuals: [])
    }

    private func historySnapshot(
        aggregateId: String = "pc-history",
        segments: [ProductConversationSegment],
        before: String? = nil,
        hasOlder: Bool = false
    ) -> ProductConversationSnapshot {
        ProductConversationSnapshot(
            product_conversation_id: aggregateId,
            close: nil,
            canonical_route: "/product-conversations/\(aggregateId)",
            requested_transcript_row_id: "latest",
            canonical_root: ProductConversationTranscriptRow(
                transcript_row_id: "root", slug: "root", title: "Root"),
            ordinary_lifecycle: .history,
            latest_transcript_row_id: "latest",
            writable_transcript_row_id: nil,
            updated_at: "2025-01-02T03:04:05Z",
            presentation: .state(displayName: "Root", presentationMode: "done"),
            work_identity: nil,
            source: nil,
            chain_qa_compatibility: nil,
            segments: segments,
            before: before,
            has_older: hasOlder)
    }

    func testAttentionMergeIncludesCoordinatorWithoutContaminatingOrdinaryList() {
        let ordinary = [conversation(id: "ordinary", aggregateId: "pc-ordinary")]
        let coordinator = conversation(
            id: "coordinator",
            aggregateId: "pc-coordinator",
            runtimeRole: "coordinator")

        let attention = AppModel.attentionConversations(
            ordinary: ordinary,
            coordinator: coordinator)

        XCTAssertEqual(ordinary.map(\.id), ["ordinary"])
        XCTAssertEqual(attention.map(\.id), ["ordinary", "coordinator"])
    }

    func testAttentionMergeReplacesDuplicateCoordinatorIdentity() {
        let stale = conversation(id: "stale", aggregateId: "pc-coordinator")
        let coordinator = conversation(
            id: "coordinator",
            aggregateId: "pc-coordinator",
            runtimeRole: "coordinator")

        let attention = AppModel.attentionConversations(
            ordinary: [stale],
            coordinator: coordinator)

        XCTAssertEqual(attention, [coordinator])
    }

    func testCoordinatorAttentionFetchUsesAuthoritativeProjection() async throws {
        let authoritative = conversation(
            id: "coordinator",
            aggregateId: "pc-coordinator",
            runtimeRole: "coordinator")

        let result = try await AppModel.coordinatorForAttention(
            rememberedId: "coordinator",
            fetch: { id in
                XCTAssertEqual(id, "coordinator")
                return authoritative
            },
            cached: { _ in XCTFail("successful fetch must not read fallback"); return nil })

        XCTAssertEqual(result, authoritative)
    }

    func testCoordinatorAttentionFetchFallsBackOnlyForTransportFailure() async throws {
        let cached = conversation(
            id: "coordinator",
            aggregateId: "pc-coordinator",
            runtimeRole: "coordinator")
        let transport = APIError.transport(underlying: URLError(.notConnectedToInternet))

        let fallback = try await AppModel.coordinatorForAttention(
            rememberedId: "coordinator",
            fetch: { _ in throw transport },
            cached: { _ in cached })
        XCTAssertEqual(fallback, cached)

        do {
            _ = try await AppModel.coordinatorForAttention(
                rememberedId: "coordinator",
                fetch: { _ in throw APIError.http(status: 500, body: "failure") },
                cached: { _ in cached })
            XCTFail("authoritative HTTP failures must propagate")
        } catch let APIError.http(status, _) {
            XCTAssertEqual(status, 500)
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }

    func testCoordinatorAttentionEvidenceFailureIsIndependent() async {
        let evidence = await AppModel.coordinatorAttentionEvidence(
            rememberedId: "coordinator",
            fetch: { _ in throw APIError.http(status: 500, body: "failure") },
            cached: { _ in XCTFail("HTTP failure must not use stale cache"); return nil })

        XCTAssertNil(evidence)
    }

    func testAPIRebuildRestartsAggregateReconciliationAfterPermanentFailure() {
        let model = AppModel()
        model.cancelAggregateReconciliationForTesting()
        XCTAssertNil(model.aggregateReconciliationId)

        model.rebuildAPIForTesting()

        XCTAssertNotNil(model.aggregateReconciliationId)
    }

    func testAggregateReconciliationRetriesTransientFailureAndApplyLoss() async {
        var attempts = 0
        var waits = 0
        let expected = [conversation(id: "ordinary", aggregateId: "pc-ordinary")]

        let result = await AppModel.fetchApplicableAggregateList(
            attempt: {
                attempts += 1
                if attempts == 1 {
                    throw APIError.transport(underlying: URLError(.networkConnectionLost))
                }
                return attempts == 2 ? nil : expected
            },
            canContinue: { true },
            waitBeforeRetry: { waits += 1 })

        XCTAssertEqual(result, expected)
        XCTAssertEqual(attempts, 3)
        XCTAssertEqual(waits, 2)
    }

    func testAggregateReconciliationCancellationStopsRetryLifecycle() async {
        let task = Task {
            await AppModel.fetchApplicableAggregateList(
                attempt: { nil },
                canContinue: { true },
                waitBeforeRetry: {
                    while !Task.isCancelled { await Task.yield() }
                    throw CancellationError()
                })
        }

        task.cancel()

        let result = await task.value
        XCTAssertNil(result)
    }

    func testRemovedAggregateProjectionIgnoresCoordinatorAndKeepsAuthoritativeOrdinaryRows() {
        let authoritative = [
            conversation(id: "ordinary", aggregateId: "pc-kept"),
            conversation(
                id: "coordinator",
                aggregateId: "pc-coordinator",
                runtimeRole: "coordinator"),
        ]

        XCTAssertEqual(
            AppModel.removedAggregateIds(
                authoritative: authoritative,
                locallyOwned: ["pc-kept", "pc-deleted", "pc-coordinator"]),
            ["pc-deleted"])
    }

    func testProductHistoryMergePreservesAggregateIdentityAndLineageOrderWithoutDuplicates() throws {
        let newer = historySnapshot(
            segments: [
                ProductConversationSegment(
                    segment_ordinal: 1,
                    transcript_row_id: "successor",
                    slug: "successor",
                    title: "Successor",
                    messages: [message("m4", sequence: 2), message("m3", sequence: 1)],
                    handoff: nil),
                ProductConversationSegment(
                    segment_ordinal: 0,
                    transcript_row_id: "root",
                    slug: "root",
                    title: "Root",
                    messages: [message("m2", sequence: 2)],
                    handoff: nil),
            ],
            before: "older-page",
            hasOlder: true)
        let handoff = ProductConversationHandoff.historical(
            predecessorTranscriptRowId: "root",
            successorTranscriptRowId: "successor",
            continuationMessageId: "boundary",
            summary: "Continued after the first transcript")
        let older = historySnapshot(segments: [
            ProductConversationSegment(
                segment_ordinal: 0,
                transcript_row_id: "root",
                slug: "root",
                title: "Root",
                messages: [message("m2", sequence: 2), message("m1", sequence: 1)],
                handoff: handoff),
        ])

        let first = try ProductHistorySnapshotStore.merging(nil, page: newer)
        let merged = try ProductHistorySnapshotStore.merging(first, page: older)

        XCTAssertEqual(merged.product_conversation_id, "pc-history")
        XCTAssertEqual(merged.segments.map(\.transcript_row_id), ["root", "successor"])
        XCTAssertEqual(merged.segments[0].messages.map(\.message_id), ["m1", "m2"])
        XCTAssertEqual(merged.segments[1].messages.map(\.message_id), ["m3", "m4"])
        XCTAssertEqual(merged.segments[0].handoff, handoff)
        XCTAssertFalse(merged.has_older)
        XCTAssertNil(merged.before)
    }

    func testProductHistoryVersionedCacheReopensOfflineAndRejectsWrongAggregate() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-product-history-tests-\(UUID().uuidString)")
        let snapshot = historySnapshot(segments: [
            ProductConversationSegment(
                segment_ordinal: 0,
                transcript_row_id: "root",
                slug: "root",
                title: "Root",
                messages: [message("cached", sequence: 1)],
                handoff: nil),
        ])
        let writer = ProductHistorySnapshotStore.writer(productConversationId: snapshot.product_conversation_id)
        let revision = writer.reserveRevision()
        let saved = await writer.save(
            CachedProductHistory(snapshot: snapshot, fetchedAt: Date()), revision: revision)
        XCTAssertTrue(saved)
        XCTAssertNil(ProductHistorySnapshotStore.load(productConversationId: "different-aggregate"))

        let model = AppModel()
        model.connectivity.setOnlineForTesting(false)
        let reopened = try await model.loadProductHistory(productConversationId: "pc-history")

        XCTAssertEqual(reopened.snapshot.product_conversation_id, "pc-history")
        XCTAssertEqual(reopened.snapshot.segments[0].messages.map(\.message_id), ["cached"])
        XCTAssertNotNil(reopened.fetchedAt)
    }

    func testCachedProductHistoryIsAvailableBeforeNetworkRefresh() async {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-product-history-tests-\(UUID().uuidString)")
        let snapshot = historySnapshot(segments: [
            ProductConversationSegment(
                segment_ordinal: 0,
                transcript_row_id: "root",
                slug: "root",
                title: "Root",
                messages: [message("cached", sequence: 1)],
                handoff: nil),
        ])
        let writer = ProductHistorySnapshotStore.writer(productConversationId: snapshot.product_conversation_id)
        let revision = writer.reserveRevision()
        let saved = await writer.save(
            CachedProductHistory(snapshot: snapshot, fetchedAt: Date()), revision: revision)
        XCTAssertTrue(saved)

        let model = AppModel()
        let cached = model.cachedProductHistory(productConversationId: "pc-history")

        XCTAssertEqual(cached?.snapshot.segments[0].messages.map(\.message_id), ["cached"])
        XCTAssertNotNil(cached?.fetchedAt)
    }

    func testProductHistoryIncrementalMergeRejectsIdentityChanges() throws {
        let first = try ProductHistorySnapshotStore.merging(
            nil,
            page: historySnapshot(segments: []))
        XCTAssertThrowsError(try ProductHistorySnapshotStore.merging(
            first,
            page: historySnapshot(aggregateId: "different", segments: []))) {
            XCTAssertEqual($0 as? ProductHistoryLoadError, .aggregateIdentityChanged)
        }
    }

    func testAuthoritativeProductHistoryRemovalCleansAggregateState() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-product-history-tests-\(UUID().uuidString)")
        let aggregateId = "pc-deleted"
        let row = conversation(id: "row-deleted", aggregateId: aggregateId)
        persistReadableSnapshot(conversation: row)
        DiskStore.save(["queued"], name: "outbox-row-deleted")
        let history = historySnapshot(aggregateId: aggregateId, segments: [])
        let writer = ProductHistorySnapshotStore.writer(productConversationId: aggregateId)
        let revision = writer.reserveRevision()
        let historySaved = await writer.save(
            CachedProductHistory(snapshot: history, fetchedAt: Date()),
            revision: revision)
        XCTAssertTrue(historySaved)

        let model = AppModel()
        model.serverURLString = "http://localhost"
        model.listStore.upsert(row)
        XCTAssertNotNil(model.session(for: row.id))

        let removed = await model.removeProductHistoryLocallyForTesting(
            productConversationId: aggregateId,
            transcriptIds: [row.id])
        XCTAssertTrue(removed)

        XCTAssertTrue(model.listStore.conversations.isEmpty)
        XCTAssertTrue(model.deletedProductHistoryIds.contains(aggregateId))
        XCTAssertNil(model.cachedProductHistory(productConversationId: aggregateId))
        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: row.id))
        XCTAssertFalse(DiskStore.listNames(prefix: "outbox-row-deleted").contains("outbox-row-deleted"))
    }

    func testAggregateDeletionEventCleansAllExactTranscriptOwnersAndConfirmation() async {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-aggregate-event-tests-\(UUID().uuidString)")
        let model = AppModel()
        model.serverURLString = "http://localhost"
        let aggregateId = "pc-deleted"
        let root = conversation(id: "root", aggregateId: aggregateId)
        let leaf = conversation(id: "leaf", aggregateId: aggregateId)
        model.listStore.upsert(root)
        model.listStore.upsert(leaf)
        persistReadableSnapshot(conversation: root)
        persistReadableSnapshot(conversation: leaf)
        DiskStore.save(["queued"], name: "outbox-root")
        DiskStore.save(["queued"], name: "outbox-leaf")
        XCTAssertNotNil(model.session(for: root.id))
        XCTAssertNotNil(model.session(for: leaf.id))
        model.installPendingProductCloseConfirmationForTesting(PendingProductCloseConfirmation(
            productConversationId: aggregateId,
            transcriptRowId: leaf.id,
            close: closeSnapshot(phase: .awaiting_stop_work_confirmation)))

        await model.handleAggregateHardDeletedForTesting(
            productConversationId: aggregateId,
            transcriptIds: [root.id, leaf.id])

        XCTAssertTrue(model.listStore.conversations.isEmpty)
        XCTAssertTrue(model.deletedProductHistoryIds.contains(aggregateId))
        XCTAssertNil(model.pendingProductCloseConfirmation)
        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: root.id))
        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: leaf.id))
        XCTAssertFalse(DiskStore.listNames(prefix: "outbox-").contains("outbox-root"))
        XCTAssertFalse(DiskStore.listNames(prefix: "outbox-").contains("outbox-leaf"))
    }

    func testRetainedProductHistoryCacheShowsAgeUntilOnlineRefreshSucceeds() {
        let now = Date()

        XCTAssertTrue(ProductHistoryCachePresentation.shouldShowAge(
            isOnline: true,
            onlineRefreshSucceeded: false,
            fetchedAt: now.addingTimeInterval(-121),
            now: now))
        XCTAssertFalse(ProductHistoryCachePresentation.shouldShowAge(
            isOnline: true,
            onlineRefreshSucceeded: true,
            fetchedAt: now.addingTimeInterval(-121),
            now: now))
        XCTAssertFalse(ProductHistoryCachePresentation.shouldShowAge(
            isOnline: true,
            onlineRefreshSucceeded: false,
            fetchedAt: now.addingTimeInterval(-119),
            now: now))
    }

    func testProductHistoryLoadKeyChangesOnOfflineToOnlineTransition() {
        let offline = ProductHistoryLoadKey(productConversationId: "product", isOnline: false)
        let online = ProductHistoryLoadKey(productConversationId: "product", isOnline: true)

        XCTAssertNotEqual(offline, online)
    }

    func testCloseCompletionGenerationsAreScopedByProduct() {
        var tracker = ProductActionGenerationTracker()
        let productA = tracker.begin(productConversationId: "product-a")
        let productB = tracker.begin(productConversationId: "product-b")

        XCTAssertTrue(tracker.isCurrent(productA, productConversationId: "product-a"))
        XCTAssertTrue(tracker.isCurrent(productB, productConversationId: "product-b"))

        let replacementA = tracker.begin(productConversationId: "product-a")
        XCTAssertFalse(tracker.isCurrent(productA, productConversationId: "product-a"))
        XCTAssertTrue(tracker.isCurrent(replacementA, productConversationId: "product-a"))
        XCTAssertTrue(tracker.isCurrent(productB, productConversationId: "product-b"))

        tracker.end(productA, productConversationId: "product-a")
        XCTAssertTrue(tracker.isCurrent(replacementA, productConversationId: "product-a"))
        tracker.end(replacementA, productConversationId: "product-a")
        XCTAssertFalse(tracker.isCurrent(replacementA, productConversationId: "product-a"))
    }

    func testArchivedTranscriptNotificationAliasRoutesToProductHistoryAggregate() {
        let model = AppModel()
        model.listStore.upsert(conversation(id: "root-row", aggregateId: "pc-history"))
        model.listStore.upsert(conversation(
            id: "latest-row", aggregateId: "pc-history", archived: true))

        XCTAssertEqual(model.notificationNavigationId(for: "root-row"), "pc-history")
        XCTAssertEqual(model.notificationNavigationId(for: "latest-row"), "pc-history")
    }

    func testPendingCloseResolutionTrackerSerializesActionsAndInvalidatesResetCompletion() {
        var tracker = ProductCloseResolutionTracker()
        let first = tracker.begin(productConversationId: "product")

        XCTAssertNotNil(first)
        XCTAssertTrue(tracker.isInFlight)
        XCTAssertNil(tracker.begin(productConversationId: "product"))

        tracker.reset()
        XCTAssertFalse(tracker.isInFlight)
        XCTAssertFalse(tracker.isCurrent(first!, productConversationId: "product"))
    }

    func testPendingCloseConfirmationKindsFollowAuthoritativePhase() {
        let stopWork = PendingProductCloseConfirmation(
            productConversationId: "product",
            transcriptRowId: "latest",
            close: closeSnapshot(phase: .awaiting_stop_work_confirmation))
        let losses = PendingProductCloseConfirmation(
            productConversationId: "product",
            transcriptRowId: "latest",
            close: closeSnapshot(phase: .awaiting_loss_confirmation))
        let repair = PendingProductCloseConfirmation(
            productConversationId: "product",
            transcriptRowId: "latest",
            close: closeSnapshot(phase: .needs_repair))
        let settling = PendingProductCloseConfirmation(
            productConversationId: "product",
            transcriptRowId: "latest",
            close: closeSnapshot(phase: .settling_active_work))

        XCTAssertEqual(stopWork.kind, .stopWork)
        XCTAssertEqual(losses.kind, .losses)
        XCTAssertEqual(repair.kind, .repair)
        XCTAssertNil(settling.kind)
    }

    func testPendingCloseConfirmationRehydratesOnlyFromAuthoritativeConfirmationPhase() {
        var snapshot = historySnapshot(segments: [])
        snapshot.close = closeSnapshot(phase: .awaiting_stop_work_confirmation)
        let pending = PendingProductCloseConfirmation(snapshot: snapshot)

        XCTAssertEqual(pending?.productConversationId, "pc-history")
        XCTAssertEqual(pending?.transcriptRowId, "latest")
        XCTAssertEqual(pending?.kind, .stopWork)

        snapshot.close = closeSnapshot(phase: .needs_repair)
        XCTAssertEqual(PendingProductCloseConfirmation(snapshot: snapshot)?.kind, .repair)

        snapshot.close = closeSnapshot(phase: .settling_active_work)
        XCTAssertNil(PendingProductCloseConfirmation(snapshot: snapshot))
    }

    func testPendingCloseReconciliationRecognizesHistoryAsCompleted() {
        let snapshot = historySnapshot(segments: [])

        XCTAssertTrue(PendingProductCloseConfirmation.isCompleted(snapshot: snapshot))
        XCTAssertNil(PendingProductCloseConfirmation(snapshot: snapshot))
    }

    func testPendingCloseReconciliationRecognizesCompletedCloseAsCompleted() {
        var snapshot = historySnapshot(segments: [])
        snapshot.ordinary_lifecycle = .open
        snapshot.close = closeSnapshot(phase: .completed)

        XCTAssertTrue(PendingProductCloseConfirmation.isCompleted(snapshot: snapshot))
        XCTAssertNil(PendingProductCloseConfirmation(snapshot: snapshot))
    }

    func testCloseLossInventoryRendersExactCategorizedItemsDeterministically() {
        let losses = [
            ProductConversationCloseLoss(
                scope: "scope-b", generation: "g", category: "untracked", identity: "notes.txt"),
            ProductConversationCloseLoss(
                scope: "scope-a", generation: "g", category: "staged", identity: "Sources/App.swift"),
        ]

        XCTAssertTrue(ProductCloseLossInventory.isComplete(losses))
        XCTAssertEqual(
            ProductCloseLossInventory.message(losses),
            "Scope: scope-a\nCategory: staged\nItem: Sources/App.swift\n\n"
                + "Scope: scope-b\nCategory: untracked\nItem: notes.txt")
        XCTAssertFalse(ProductCloseLossInventory.isComplete([]))
        XCTAssertFalse(ProductCloseLossInventory.isComplete([
            ProductConversationCloseLoss(
                scope: "scope", generation: "g", category: "tracked", identity: ""),
        ]))
    }

    func testClearCacheClearsPendingCloseAndInFlightResolution() async {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-close-reset-tests-\(UUID().uuidString)")
        let model = AppModel()
        let pending = PendingProductCloseConfirmation(
            productConversationId: "product",
            transcriptRowId: "latest",
            close: closeSnapshot(phase: .awaiting_stop_work_confirmation))
        model.installPendingProductCloseConfirmationForTesting(pending, resolving: true)
        XCTAssertNotNil(model.pendingProductCloseConfirmation)
        XCTAssertTrue(model.isResolvingPendingProductClose)

        await model.clearCache()

        XCTAssertNil(model.pendingProductCloseConfirmation)
        XCTAssertFalse(model.isResolvingPendingProductClose)
    }

    func testRepairNotNowKeepsAggregateMessageAdmissionFenced() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-close-repair-fence-tests-\(UUID().uuidString)")
        let model = AppModel()
        model.rebuildAPIForTesting()
        model.connectivity.setOnlineForTesting(true)
        let row = conversation(id: "latest", aggregateId: "product")
        model.listStore.upsert(row)
        let session = try XCTUnwrap(model.session(for: row.id))
        model.installPendingProductCloseConfirmationForTesting(PendingProductCloseConfirmation(
            productConversationId: "product",
            transcriptRowId: row.id,
            close: closeSnapshot(phase: .needs_repair)))

        await model.resolvePendingProductCloseConfirmation(confirm: false)

        XCTAssertNil(model.pendingProductCloseConfirmation)
        XCTAssertTrue(session.isArchiving)
        XCTAssertFalse(session.acceptsConversationActions)
        XCTAssertFalse(session.acceptsChatMessage)
    }

    func testActiveCloseFenceAppliesToSessionCreatedAfterRehydration() throws {
        let model = AppModel()
        model.rebuildAPIForTesting()
        let row = conversation(id: "latest", aggregateId: "product")
        model.listStore.upsert(row)
        model.fenceProductCloseForTesting(productConversationId: "product", fenced: true)

        let session = try XCTUnwrap(model.session(for: row.id))

        XCTAssertTrue(session.isArchiving)
        XCTAssertFalse(session.acceptsConversationActions)
    }

    func testPendingCloseConfirmationBlocksPersistedAggregateOutboxWithoutRequest() async {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-close-outbox-tests-\(UUID().uuidString)")
        DiskStore.save(["queued"], name: "outbox-latest")
        let model = AppModel()
        model.serverURLString = "http://127.0.0.1:1"
        model.connectivity.setOnlineForTesting(true)
        model.installPendingProductCloseConfirmationForTesting(PendingProductCloseConfirmation(
            productConversationId: "product",
            transcriptRowId: "latest",
            close: closeSnapshot(phase: .awaiting_stop_work_confirmation)))

        await model.resolvePendingProductCloseConfirmation(confirm: true)

        XCTAssertEqual(
            model.lastActionError,
            "This conversation has queued or unreadable messages. Resolve them before closing.")
        XCTAssertNotNil(model.pendingProductCloseConfirmation)
        XCTAssertFalse(model.isResolvingPendingProductClose)
    }

    func testOfflineCloseConfirmationFailsImmediatelyWithExplanation() async {
        let model = AppModel()
        model.connectivity.setOnlineForTesting(false)

        await model.resolvePendingProductCloseConfirmation(confirm: true)

        XCTAssertEqual(
            model.lastActionError,
            "Resolving a Close confirmation needs a connection — reconnect and try again.")
    }

    func testHistoryGenerationInvalidatesOnlyMatchingProduct() {
        var tracker = ProductActionGenerationTracker()
        let productA = tracker.begin(productConversationId: "product-a")
        let productB = tracker.begin(productConversationId: "product-b")

        _ = tracker.begin(productConversationId: "product-a")

        XCTAssertFalse(tracker.isCurrent(productA, productConversationId: "product-a"))
        XCTAssertTrue(tracker.isCurrent(productB, productConversationId: "product-b"))
    }

    func testProductHistoryHandoffDisplaySummaryCoversBothKinds() {
        let historical = ProductConversationHandoff.historical(
            predecessorTranscriptRowId: "root",
            successorTranscriptRowId: "next",
            continuationMessageId: "handoff",
            summary: "Historical summary")
        let completed = ProductConversationHandoff.completed(
            predecessorTranscriptRowId: "root",
            successorTranscriptRowId: "next",
            continuationMessageId: "handoff",
            acceptedSuccessorMessageId: "accepted",
            summary: "Completed summary")

        XCTAssertEqual(historical.displaySummary, "Historical summary")
        XCTAssertEqual(completed.displaySummary, "Completed summary")
    }

    func testBackgroundIntegrationPreservesAuthoritativeAggregateIdentityAfterLegacyCache() {
        let model = AppModel()
        let aggregateProjection = conversation(
            id: "latest-row",
            aggregateId: "pc-1",
            slug: "canonical-root",
            title: "Canonical Title",
            updatedAt: "2025-01-02T03:04:05Z")
        let liveTranscriptUpdate = conversation(
            id: "newer-row",
            slug: "transcript-slug",
            title: "Transcript Title",
            updatedAt: "2025-01-02T05:04:05Z")

        let merged = model.integrateBackgroundConversationUpdate(
            existing: aggregateProjection,
            update: liveTranscriptUpdate)

        XCTAssertEqual(merged.product_conversation_id, "pc-1")
        XCTAssertEqual(merged.aggregateIdentity, "pc-1")
        XCTAssertEqual(merged.id, "latest-row")
    }

    func testBackgroundIntegrationPreservesCanonicalRootMetadataAcrossLiveUpdate() {
        let model = AppModel()
        let aggregateProjection = conversation(
            id: "latest-row",
            aggregateId: "pc-1",
            slug: "canonical-root",
            title: "Canonical Title",
            taskTitle: "Canonical Task",
            archived: false,
            mode: "working",
            updatedAt: "2025-01-02T03:04:05Z")
        let liveTranscriptUpdate = conversation(
            id: "newer-row",
            slug: "ephemeral-transcript-slug",
            title: "Ephemeral Transcript Title",
            taskTitle: nil,
            archived: true,
            mode: "needs_action",
            updatedAt: "2025-01-02T06:04:05Z")

        let merged = model.integrateBackgroundConversationUpdate(
            existing: aggregateProjection,
            update: liveTranscriptUpdate)

        XCTAssertEqual(merged.product_conversation_id, "pc-1")
        XCTAssertEqual(merged.slug, "canonical-root")
        XCTAssertEqual(merged.title, "Canonical Title")
        XCTAssertEqual(merged.task_title, "Canonical Task")
        XCTAssertEqual(merged.archived, false)
        XCTAssertEqual(merged.presentation_mode, "needs_action")
        XCTAssertEqual(merged.id, "latest-row")
    }

    func testBackgroundIntegrationIgnoresDivergentSuccessorTaskTitle() {
        let model = AppModel()
        let aggregateProjection = conversation(
            id: "latest-row",
            aggregateId: "pc-1",
            slug: "canonical-root",
            title: "Canonical Title",
            taskTitle: "Canonical Task")
        let liveTranscriptUpdate = conversation(
            id: "successor-row",
            slug: "successor-slug",
            title: "Successor Title",
            taskTitle: "Successor Task Title")

        let merged = model.integrateBackgroundConversationUpdate(
            existing: aggregateProjection,
            update: liveTranscriptUpdate)

        XCTAssertEqual(merged.title, "Canonical Title")
        XCTAssertEqual(merged.task_title, "Canonical Task")
    }


    func testColdRefreshDoesNotReinjectCoordinatorSnapshotIntoAuthoritativeList() async {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let coordinator = conversation(
            id: "coordinator-row",
            slug: "fleet",
            title: "Fleet",
            runtimeRole: "coordinator")
        persistReadableSnapshot(conversation: coordinator)
        UserDefaults.standard.set("coordinator-row", forKey: "phoenix.coordinatorConversationId")

        let model = AppModel()

        XCTAssertEqual(model.listStore.conversations.filter(\.isCoordinator).count, 0)
        let coordinatorId = await model.openCoordinator()
        XCTAssertEqual(coordinatorId, "coordinator-row")
    }

    func testOfflineNotificationNavigationUsesCachedAggregateMember() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let predecessor = conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        persistReadableSnapshot(conversation: predecessor)
        let model = AppModel()
        model.listStore.upsert(predecessor)
        model.listStore.upsert(conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        model.connectivity.setOnlineForTesting(false)

        let resolved = model.resolvedNavigationConversationId(
            aggregateId: model.listStore.aggregateId(forTranscriptRowId: "row-2"),
            latestTranscriptRowId: "row-2")

        XCTAssertEqual(resolved, "row-1")
    }
    func testOfflineHandoffNavigationUsesCachedAggregateMember() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let predecessor = conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        persistReadableSnapshot(conversation: predecessor)
        let model = AppModel()
        model.listStore.upsert(predecessor)
        model.listStore.upsert(conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        model.connectivity.setOnlineForTesting(false)

        let resolved = model.resolvedNavigationConversationId(
            aggregateId: model.listStore.aggregateId(forTranscriptRowId: "row-2"),
            latestTranscriptRowId: "row-2")

        XCTAssertEqual(resolved, "row-1")
    }

    func testOfflineNavigationUsesCachedAggregateMemberWhenLatestSnapshotMissing() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let predecessor = conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        persistReadableSnapshot(conversation: predecessor)
        let model = AppModel()
        model.listStore.upsert(predecessor)
        model.listStore.upsert(conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        let aggregateConversation = model.listStore.conversations.first!
        model.connectivity.setOnlineForTesting(false)

        XCTAssertEqual(model.navigationConversationId(for: aggregateConversation), "row-1")
    }

    func testOfflineNavigationUsesCachedAggregateMemberAfterRestart() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let predecessor = conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        persistReadableSnapshot(conversation: predecessor)
        let first = AppModel()
        first.listStore.upsert(predecessor)
        first.listStore.applyExternal(
            [conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root")],
            startedAt: first.listStore.externalRefreshToken())

        let reloaded = AppModel()
        reloaded.connectivity.setOnlineForTesting(false)
        let aggregateConversation = reloaded.listStore.conversations.first!

        XCTAssertEqual(reloaded.navigationConversationId(for: aggregateConversation), "row-1")
    }
}
