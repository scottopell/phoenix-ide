import XCTest

@testable import PhoenixMobile

private struct TestPersistedOutboxStore {
    var visibleByTranscriptRowId: [String: Bool] = [:]

    func contents(for transcriptRowId: String) -> Outbox.StoredContents {
        visibleByTranscriptRowId[transcriptRowId] == true ? .hasVisibleEntries : .empty
    }
}

@MainActor
final class ProductConversationDetailModelTests: XCTestCase {

    private func makeAPI() -> PhoenixAPI {
        PhoenixAPI(baseURL: URL(string: "https://example.com")!, password: nil, allowSelfSigned: true)!
    }

    private func makeSession(id: String) -> ConversationSession {
        ConversationSession(conversationId: id, api: makeAPI(), connectivity: ConnectivityMonitor())
    }

    private func snapshot(
        lifecycle: ProductConversationOrdinaryLifecycle = .open,
        latest: String = "row-2",
        writable: String? = "row-2",
        before: String? = nil,
        hasOlder: Bool = false
    ) -> ProductConversationSnapshot {
        ProductConversationSnapshot(
            product_conversation_id: "pc-1",
            close: nil,
            canonical_route: "/product-conversations/pc-1",
            requested_transcript_row_id: latest,
            canonical_root: .init(transcript_row_id: "row-1", slug: "root", title: "Root"),
            ordinary_lifecycle: lifecycle,
            latest_transcript_row_id: latest,
            writable_transcript_row_id: writable,
            updated_at: "2025-01-02T03:04:05Z",
            presentation: .state(displayName: "Root", presentationMode: "working"),
            work_identity: nil,
            source: nil,
            chain_qa_compatibility: nil,
            segments: [
                .init(
                    segment_ordinal: 0,
                    transcript_row_id: "row-1",
                    slug: "root",
                    title: "Root",
                    messages: [
                        .init(
                            message_id: "m-1",
                            conversation_id: "row-1",
                            sequence_id: 4,
                            message_type: "user",
                            content: .object(["text": .string("before")]),
                            display_data: nil,
                            created_at: "2025-01-02T03:04:05Z")
                    ],
                    handoff: .historical(
                        predecessorTranscriptRowId: "row-1",
                        successorTranscriptRowId: "row-2",
                        continuationMessageId: "m-cont",
                        summary: "summary")),
                .init(
                    segment_ordinal: 1,
                    transcript_row_id: "row-2",
                    slug: "next",
                    title: "Next",
                    messages: [
                        .init(
                            message_id: "m-2",
                            conversation_id: "row-2",
                            sequence_id: 1,
                            message_type: "agent",
                            content: .object(["text": .string("after")]),
                            display_data: nil,
                            created_at: "2025-01-02T03:04:06Z")
                    ],
                    handoff: nil),
            ],
            before: before,
            has_older: hasOlder)
    }

    func testPrefersWritableTranscriptSessionWithoutRetargetingExistingSessions() {
        let row1 = makeSession(id: "row-1")
        let row2 = makeSession(id: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in
                switch id {
                case "row-1": row1
                case "row-2": row2
                default: nil
                }
            })

        model.applyForTesting(snapshot())

        XCTAssertTrue(model.actionSession === row2)
        XCTAssertEqual(model.actionTranscriptRowId, "row-2")
        XCTAssertEqual(row2.conversationId, "row-2")
    }

    func testHistoryOnlyDisablesGlobalMutationsButOpenWithoutWritableStillAllowsLifecycleActions() {
        let row2 = makeSession(id: "row-2")
        row2.receive(.initSnapshot(.init(
            conversation: try! conversation(id: "row-2", state: "{\"type\":\"needs_input\",\"task_kind\":\"edit\"}"),
            messages: [], agentWorking: false, presentationMode: "needs_action", lastSequenceId: 1,
            pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? row2 : nil })

        model.applyForTesting(snapshot(lifecycle: .open, writable: nil))
        XCTAssertFalse(model.canSendChat)
        XCTAssertFalse(model.isHistoryReadOnly)
        XCTAssertTrue(model.canMutateLifecycle)

        model.applyForTesting(snapshot(lifecycle: .history, writable: nil))
        XCTAssertTrue(model.isHistoryReadOnly)
        XCTAssertFalse(model.canMutateLifecycle)
    }

    func testComposedMessagesPreserveSegmentBoundaryOrderWhenSequenceResets() {
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil })

        model.applyForTesting(snapshot())

        let items = model.transcriptItems
        XCTAssertEqual(items.count, 3)
        XCTAssertEqual(items.map(\.debugLabel), ["message:m-1", "handoff:summary", "message:m-2"])
    }

    func testSelectingHistoricalSegmentDoesNotMoveWritableDelegate() {
        let row1 = makeSession(id: "row-1")
        let row2 = makeSession(id: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-1" ? row1 : (id == "row-2" ? row2 : nil) })

        model.applyForTesting(snapshot())
        model.selectTranscriptRow(id: "row-1")

        XCTAssertEqual(model.selectedTranscriptRowId, "row-1")
        XCTAssertEqual(model.actionTranscriptRowId, "row-2")
        XCTAssertTrue(model.actionSession === row2)
    }

    func testLiveOverlayReplacesByIdentityAndSuppressesBoundaryContinuation() {
        let row2 = makeSession(id: "row-2")
        row2.receive(.initSnapshot(.init(
            conversation: try! conversation(id: "row-2"),
            messages: [
                .init(message_id: "m-2", conversation_id: "row-2", sequence_id: 1, message_type: "agent", content: .object(["text": .string("after-live")]), display_data: nil, created_at: "2025-01-02T03:04:07Z"),
                .init(message_id: "m-cont", conversation_id: "row-2", sequence_id: 2, message_type: "user", content: .object(["text": .string("suppressed")]), display_data: nil, created_at: "2025-01-02T03:04:08Z")
            ], agentWorking: false, presentationMode: "working", lastSequenceId: 2,
            pendingAnchorSequenceId: 2, pendingEvents: [], pendingTruncated: false)))
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? row2 : nil })

        model.applyForTesting(snapshot())
        let labels = model.transcriptItems.map(\.debugLabel)
        XCTAssertEqual(labels, ["message:m-1", "handoff:summary", "message:m-2"])
    }

    func testToolUseIndexIncludesHistoricalToolUseBlocksAcrossSegments() {
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil })
        var snap = snapshot()
        snap.segments[0].messages = [
            .init(message_id: "tool-msg", conversation_id: "row-1", sequence_id: 4, message_type: "agent", content: .array([
                .object(["type": .string("tool_use"), "id": .string("tu-1"), "name": .string("read_file"), "input": .object(["path": .string("a")])]),
                .object(["type": .string("tool_result"), "tool_use_id": .string("tu-1"), "content": .string("ok")])
            ]), display_data: nil, created_at: "2025-01-02T03:04:05Z")
        ]

        model.applyForTesting(snap)

        XCTAssertEqual(model.composedToolUseIndex["tu-1"]?.name, "read_file")
    }

    func testOutboxProjectionIncludesPredecessorEntriesWithOwningSession() async {
        let connectivity = ConnectivityMonitor()
        connectivity.setOnlineForTesting(false)
        let row1 = ConversationSession(conversationId: "row-1", api: makeAPI(), connectivity: connectivity)
        let row2 = ConversationSession(conversationId: "row-2", api: makeAPI(), connectivity: connectivity)
        row1.receive(.initSnapshot(.init(
            conversation: try! conversation(id: "row-1"),
            messages: [], agentWorking: false, presentationMode: "idle", lastSequenceId: 1,
            pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
        _ = await row1.send(text: "pending predecessor")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-1" ? row1 : (id == "row-2" ? row2 : nil) },
            existingSession: { id in id == "row-1" ? row1 : (id == "row-2" ? row2 : nil) })

        model.applyForTesting(snapshot())

        let targetLocalIds = Set(row1.outbox.visibleEntries.map(\.localId))
        let projections = model.outboxProjections.filter { targetLocalIds.contains($0.entry.localId) }
        XCTAssertEqual(Set(projections.map(\.entry.localId)), targetLocalIds)
        XCTAssertEqual(Set(projections.map(\.transcriptRowId)), ["row-1"])
        XCTAssertTrue(({ if case .interactive(let session) = projections[0].actionPolicy { session === row1 } else { false } })())
    }

    func testTopologyInvalidationBurstUsesSingleFlightCoalescingWithoutSleep() async {
        final class Counter {
            var refreshCalls = 0
            weak var model: ProductConversationDetailModel?
        }
        let counter = Counter()
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil },
            loadSnapshot: { _, _ in self.snapshot() },
            didStartRefresh: { cause in
                _ = cause
                counter.refreshCalls += 1
                if counter.refreshCalls == 1 {
                    counter.model?.invalidateAggregateTopologyForTesting(
                        ProductConversationTopologyInvalidation(
                            transcriptRowId: "row-2",
                            aggregateIdentity: "pc-1",
                            reason: .awaitingContinuation))
                    counter.model?.invalidateAggregateTopologyForTesting(
                        ProductConversationTopologyInvalidation(
                            transcriptRowId: "row-2",
                            aggregateIdentity: "pc-1",
                            reason: .handedOff(successorConversationId: "row-3")))
                }
            })
        counter.model = model

        model.invalidateAggregateTopologyForTesting(
            ProductConversationTopologyInvalidation(
                transcriptRowId: "row-2",
                aggregateIdentity: "pc-1",
                reason: .contextExhausted))

        while counter.refreshCalls < 2 { await Task.yield() }
        XCTAssertEqual(counter.refreshCalls, 2)
    }

    func testRepeatedIdenticalInvalidationDoesNotRequireMoreThanOneFollowupRefresh() async {
        final class Counter { var refreshCalls = 0 }
        let counter = Counter()
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil },
            loadSnapshot: { _, _ in self.snapshot() },
            didStartRefresh: { _ in counter.refreshCalls += 1 })

        let invalidation = ProductConversationTopologyInvalidation(
            transcriptRowId: "row-2",
            aggregateIdentity: "pc-1",
            reason: .awaitingContinuation)
        model.invalidateAggregateTopologyForTesting(invalidation)
        model.invalidateAggregateTopologyForTesting(invalidation)

        while counter.refreshCalls < 1 { await Task.yield() }
        XCTAssertEqual(counter.refreshCalls, 1)
    }

    func testAggregateSnapshotAuthorityWinsOverHandoffHintForSuccessorRebind() {
        let row2 = makeSession(id: "row-2")
        let row4 = makeSession(id: "row-4")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? row2 : (id == "row-4" ? row4 : nil) })

        model.handleSessionEvent(
            transcriptRowId: "row-2",
            generation: 0,
            event: ProductConversationSessionEvent.aggregateTopologyInvalidated(
                ProductConversationTopologyInvalidation(
                    transcriptRowId: "row-2",
                    aggregateIdentity: "pc-1",
                    reason: .handedOff(successorConversationId: "row-3"))))
        model.applyForTesting(snapshot(latest: "row-4", writable: "row-4"))

        XCTAssertEqual(model.actionTranscriptRowId, "row-4")
        XCTAssertTrue(model.actionSession === row4)
    }

    func testPaginationAdvancesCursorFromOlderPage() async {
        let pages = [
            snapshot(before: "cursor-1", hasOlder: true),
            snapshot(before: "cursor-2", hasOlder: false),
        ]
        final class Box { var calls: [String?] = [] ; var index = 0 }
        let box = Box()
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil },
            loadSnapshot: { _, before in
                box.calls.append(before)
                defer { box.index += 1 }
                return pages[min(box.index, pages.count - 1)]
            })

        await model.refresh(cause: .manual)
        XCTAssertEqual(model.olderCursor, "cursor-1")
        await model.loadOlder()
        XCTAssertEqual(box.calls, [nil, "cursor-1"])
        XCTAssertEqual(model.olderCursor, "cursor-2")
        XCTAssertFalse(model.hasOlder)
    }

    func testHistoryDoesNotCreateSessionsForLatestOrSelectionWithoutExistingSession() {
        final class Counter { var created: [String] = [] }
        let counter = Counter()
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in counter.created.append(id); return self.makeSession(id: id) })

        model.applyForTesting(snapshot(lifecycle: .history, writable: nil))
        _ = model.stateDetailSession
        _ = model.selectedTranscriptSession

        XCTAssertTrue(counter.created.isEmpty)
    }

    func testOpenWithoutWritableUsesLatestSessionForActionsButNotChat() {
        let latest = makeSession(id: "row-2")
        latest.receive(.initSnapshot(.init(
            conversation: try! conversation(id: "row-2", state: "{\"type\":\"needs_input\",\"task_kind\":\"edit\"}"),
            messages: [], agentWorking: false, presentationMode: "needs_action", lastSequenceId: 1,
            pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? latest : nil },
            existingSession: { id in id == "row-2" ? latest : nil })

        model.applyForTesting(snapshot(lifecycle: .open, writable: nil))

        XCTAssertFalse(model.canSendChat)
        XCTAssertTrue(model.canMutateLifecycle)
        XCTAssertTrue(model.stateDetailSession === latest)
    }

    func testAggregateIdentityInvalidatesBothOldAndNewOwners() {
        let model = ProductConversationDetailModel(
            aggregateId: "pc-old",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil })

        XCTAssertTrue(model.invalidatesAggregateForTesting(.init(
            transcriptRowId: "row-2",
            aggregateIdentity: "pc-new",
            reason: .aggregateIdentityChanged(previous: "pc-old", current: "pc-new"))))
    }

    func testLateCancelledLoadCannotClearNewFlight() async {
        final class Gate {
            var firstStarted = false
            var firstCanFinish = false
            var secondCount = 0
        }
        let gate = Gate()
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil },
            loadSnapshot: { _, _ in
                if !gate.firstStarted {
                    gate.firstStarted = true
                    while !gate.firstCanFinish { await Task.yield() }
                    return self.snapshot(latest: "row-2", writable: "row-2")
                }
                gate.secondCount += 1
                return self.snapshot(latest: "row-3", writable: "row-3")
            })

        Task { await model.refresh(cause: .manual) }
        while !gate.firstStarted { await Task.yield() }
        model.stop()
        Task { await model.refresh(cause: .manual) }
        gate.firstCanFinish = true
        while gate.secondCount < 1 { await Task.yield() }
        XCTAssertEqual(model.latestTranscriptRowId, "row-3")
    }

    func testLoadOlderThenRefreshPreservesDeepestCursor() async {
        let initial = snapshot(before: "cursor-1", hasOlder: true)
        let older = snapshot(before: "cursor-2", hasOlder: true)
        let refreshed = snapshot(before: "cursor-1", hasOlder: true)
        final class Box { var calls: [String?] = [] ; var index = 0 }
        let box = Box()
        let responses = [initial, older, refreshed]
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil },
            loadSnapshot: { _, before in
                box.calls.append(before)
                defer { box.index += 1 }
                return responses[min(box.index, responses.count - 1)]
            })

        await model.refresh(cause: .manual)
        await model.loadOlder()
        await model.refresh(cause: .manual)

        XCTAssertEqual(box.calls, [nil, "cursor-1", nil])
        XCTAssertEqual(model.olderCursor, "cursor-2")
        XCTAssertTrue(model.hasOlder)
    }

    func testColdRestartDiscoversPersistedPredecessorOutboxWithoutPreexistingSession() async throws {
        final class SessionRegistry {
            var sessions: [String: ConversationSession] = [:]
            var created: [String] = []
        }
        try await withScopedDiskStoreDirectory {
            let connectivity = ConnectivityMonitor()
            connectivity.setOnlineForTesting(false)
            let registry = SessionRegistry()
            let row1 = ConversationSession(conversationId: "row-1", api: self.makeAPI(), connectivity: connectivity)
            row1.receive(.initSnapshot(.init(
                conversation: try! self.conversation(id: "row-1"),
                messages: [], agentWorking: false, presentationMode: "idle", lastSequenceId: 1,
                pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
            _ = await row1.send(text: "pending predecessor")
            let persisted = TestPersistedOutboxStore(visibleByTranscriptRowId: ["row-1": true])
            let model = ProductConversationDetailModel(
                aggregateId: "pc-1",
                api: self.makeAPI(),
                connectivity: connectivity,
                sessionProvider: { id in
                    registry.created.append(id)
                    if registry.sessions[id] == nil, id == "row-1" {
                        registry.sessions[id] = row1
                    }
                    return registry.sessions[id]
                },
                existingSession: { id in registry.sessions[id] },
                persistedOutboxContents: { persisted.contents(for: $0) })

            model.applyForTesting(self.snapshot())
            XCTAssertEqual(model.outboxProjections.count, 1)
            XCTAssertEqual(model.outboxProjections[0].transcriptRowId, "row-1")
            XCTAssertEqual(registry.created, ["row-1", "row-2", "row-2"])
            _ = model.outboxProjections
            _ = model.transcriptItems
            _ = model.displayTitle
            XCTAssertEqual(registry.created, ["row-1", "row-2", "row-2"])
        }
    }

    func testTopologyGenerationChangeResetsToFreshCursorChain() async {
        let pages = [
            snapshot(before: "cursor-1", hasOlder: true),
            snapshot(before: "cursor-older", hasOlder: true),
            snapshot(latest: "row-3", writable: "row-3", before: "cursor-fresh", hasOlder: true),
        ]
        final class Box { var index = 0 }
        let box = Box()
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil },
            loadSnapshot: { _, _ in defer { box.index += 1 }; return pages[min(box.index, pages.count - 1)] })

        await model.refresh(cause: .manual)
        await model.loadOlder()
        await model.refresh(cause: .delegateConversationChanged)

        XCTAssertEqual(model.olderCursor, "cursor-fresh")
        XCTAssertEqual(model.latestTranscriptRowId, "row-3")
    }

    func testDelegatedLifecycleSessionConnectionDrivesActionConnectivityWhenNoChatOwner() {
        let latest = makeSession(id: "row-2")
        latest.receive(.initSnapshot(.init(
            conversation: try! conversation(id: "row-2", state: "{\"type\":\"needs_input\",\"task_kind\":\"edit\"}"),
            messages: [], agentWorking: false, presentationMode: "needs_action", lastSequenceId: 1,
            pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? latest : nil },
            existingSession: { id in id == "row-2" ? latest : nil })

        model.applyForTesting(snapshot(lifecycle: .open, writable: nil))
        model.applyOwnerEventForTesting(.connectionChanged(.offline))

        XCTAssertFalse(model.canSendChat)
        XCTAssertEqual(model.currentOwnerConnection, .offline)
        XCTAssertFalse(model.delegatedConnectivityAllowsActions)
    }

    func testDismissDelegatedErrorTargetsCurrentLifecycleOwnerAndDoesNotResurrectOnRefresh() async {
        let latest = makeSession(id: "row-2")
        latest.receive(.errorEvent(seq: 2, message: "boom", retryable: false))
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? latest : nil },
            existingSession: { id in id == "row-2" ? latest : nil },
            loadSnapshot: { _, _ in self.snapshot(lifecycle: .open, writable: nil) })

        model.applyForTesting(snapshot(lifecycle: .open, writable: nil))
        model.applyOwnerEventForTesting(.errorToastChanged("boom"))
        model.dismissDelegatedError()
        await model.refresh(cause: .manual)

        XCTAssertNil(latest.lastErrorToast)
        XCTAssertNil(model.lastDelegatedError)
    }

    func testInitialAggregateLoadFailureFallsBackToCachedMemberSession() async throws {
        try await withScopedDiskStoreDirectory {
            let cached = self.makeSession(id: "row-2")
            cached.receive(.initSnapshot(.init(
                conversation: try! self.conversation(id: "row-2"),
                messages: [try! self.message(id: "m-cache", type: "agent", content: "{\"text\":\"cached\"}")],
                agentWorking: false, presentationMode: "idle", lastSequenceId: 1,
                pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
            _ = await cached.send(text: "pending cached")
            let persisted = TestPersistedOutboxStore(visibleByTranscriptRowId: ["row-2": true])
            let model = ProductConversationDetailModel(
                aggregateId: "pc-1",
                initialTranscriptRowId: "row-2",
                api: self.makeAPI(),
                connectivity: ConnectivityMonitor(),
                sessionProvider: { id in id == "row-2" ? cached : nil },
                existingSession: { id in id == "row-2" ? cached : nil },
                persistedOutboxContents: { persisted.contents(for: $0) },
                hasCachedSnapshot: { $0 == "row-2" },
                loadSnapshot: { _, _ in throw URLError(.cannotConnectToHost) })

            await model.start()

            XCTAssertTrue(model.fallbackSession === cached)
            XCTAssertEqual(model.selectedTranscriptRowId, "row-2")
            XCTAssertEqual(model.transcriptItems.map(\.debugLabel), ["message:m-cache"])
        }
    }

    func testHistoryNeverOverlaysExistingSessionMessagesOverSnapshotAuthority() {
        let existing = makeSession(id: "row-2")
        existing.receive(.initSnapshot(.init(
            conversation: try! conversation(id: "row-2"),
            messages: [
                .init(message_id: "m-live", conversation_id: "row-2", sequence_id: 9, message_type: "agent", content: .object(["text": .string("live")]), display_data: nil, created_at: "2025-01-02T03:04:09Z")
            ], agentWorking: false, presentationMode: "idle", lastSequenceId: 9,
            pendingAnchorSequenceId: 9, pendingEvents: [], pendingTruncated: false)))
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? existing : nil },
            existingSession: { id in id == "row-2" ? existing : nil })

        model.applyForTesting(snapshot(lifecycle: .history, writable: nil))

        XCTAssertEqual(model.transcriptItems.map(\.debugLabel), ["message:m-1", "handoff:summary", "message:m-2"])
    }

    func testChatCapabilityInvalidationTriggersRefreshAndRestoresWritableComposerState() async {
        final class Box { var refreshes = 0 }
        let box = Box()
        let refreshed = snapshot(lifecycle: .open, writable: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil },
            loadSnapshot: { _, _ in refreshed },
            didStartRefresh: { _ in box.refreshes += 1 })

        model.applyForTesting(snapshot(lifecycle: .open, writable: nil))
        model.invalidateAggregateTopologyForTesting(.init(
            transcriptRowId: "row-2",
            aggregateIdentity: "pc-1",
            reason: .awaitingContinuation))
        while box.refreshes < 1 { await Task.yield() }

        XCTAssertTrue(model.canSendChat)
        XCTAssertEqual(model.actionTranscriptRowId, "row-2")
    }

    func testStartWithRetainedSnapshotKeepsCachedOwnerLiveWhenRefreshFails() async {
        let cached = makeSession(id: "row-2")
        cached.receive(.initSnapshot(.init(
            conversation: try! conversation(id: "row-2"),
            messages: [], agentWorking: false, presentationMode: "idle", lastSequenceId: 1,
            pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
        cached.receive(.errorEvent(seq: 2, message: "cached error", retryable: true))
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? cached : nil },
            existingSession: { id in id == "row-2" ? cached : nil },
            loadSnapshot: { _, _ in throw URLError(.timedOut) })

        model.applyForTesting(snapshot(lifecycle: .open, writable: "row-2"))
        await model.start()

        XCTAssertEqual(model.lastDelegatedError, "cached error")
    }

    func testRepeatedProjectionReadsDoNotCreateAdditionalSessions() {
        final class Counter { var created: [String] = [] }
        let counter = Counter()
        let row2 = makeSession(id: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in
                counter.created.append(id)
                return id == "row-2" ? row2 : nil
            },
            existingSession: { id in id == "row-2" ? row2 : nil })

        model.applyForTesting(snapshot(latest: "row-2", writable: "row-2"))
        let createdAfterApply = counter.created

        _ = model.actionSession
        _ = model.lifecycleSession
        _ = model.stateDetailSession
        _ = model.selectedTranscriptSession
        _ = model.canMutateLifecycle
        _ = model.currentOwnerSession
        _ = model.outboxProjections
        _ = model.transcriptItems

        XCTAssertEqual(counter.created, createdAfterApply)
    }

    func testInitialFallbackRequiresCachedSnapshot() async {
        let uncached = makeSession(id: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            initialTranscriptRowId: "row-2",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? uncached : nil },
            existingSession: { _ in nil },
            hasCachedSnapshot: { _ in false },
            loadSnapshot: { _, _ in throw APIError.transport(underlying: URLError(.notConnectedToInternet)) })

        await model.start()

        XCTAssertNil(model.fallbackSession)
        XCTAssertEqual(model.loadError, APIError.transport(underlying: URLError(.notConnectedToInternet)).errorDescription)
    }

    func testAggregateNotFoundTriggersOwnedCleanup() async {
        final class Box {
            var cleanedTranscriptRowId: String?
        }
        let box = Box()
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            initialTranscriptRowId: "row-2",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil },
            existingSession: { _ in nil },
            handleDefinitiveNotFound: { transcriptRowId in
                box.cleanedTranscriptRowId = transcriptRowId
            },
            loadSnapshot: { _, _ in throw APIError.http(status: 404, body: "gone") })

        await model.start()

        XCTAssertEqual(box.cleanedTranscriptRowId, "row-2")
        XCTAssertNil(model.snapshot)
        XCTAssertNil(model.fallbackSession)
        XCTAssertNil(model.loadError)
    }

    func testSeededPersistedOutboxFallbackActivatesOnInitialLoadFailure() async {
        let row1 = makeSession(id: "row-1")
        let row2 = makeSession(id: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            initialTranscriptRowId: nil,
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in
                switch id {
                case "row-1": row1
                case "row-2": row2
                default: nil
                }
            },
            existingSession: { _ in nil },
            persistedOutboxContents: { transcriptRowId in
                transcriptRowId == "row-2" ? .hasVisibleEntries : .empty
            },
            loadSnapshot: { _, _ in throw APIError.transport(underlying: URLError(.notConnectedToInternet)) })

        model.seedPersistedOutboxFallbackOwnerForTesting("row-2")
        await model.start()

        XCTAssertNil(model.snapshot)
        XCTAssertEqual(model.fallbackSession?.conversationId, "row-2")
        XCTAssertEqual(model.selectedTranscriptRowId, "row-2")
        XCTAssertTrue(model.outboxProjections.isEmpty)
    }

    func testPersistedOutboxRemovalReleasesFallbackOwnerWhenNoLongerNeeded() async {
        final class Box {
            var persisted = TestPersistedOutboxStore(visibleByTranscriptRowId: ["row-1": true])
            var sessions: [String: ConversationSession] = [:]
            var created: [String] = []
        }
        let box = Box()
        let row1 = makeSession(id: "row-1")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in
                box.created.append(id)
                if box.sessions[id] == nil, id == "row-1" { box.sessions[id] = row1 }
                return box.sessions[id]
            },
            existingSession: { box.sessions[$0] },
            persistedOutboxContents: { box.persisted.contents(for: $0) })

        await model.start()
        model.applyForTesting(snapshot())
        XCTAssertTrue(model.fallbackSession === row1)
        XCTAssertEqual(box.created, ["row-1", "row-2", "row-2"])

        box.persisted = TestPersistedOutboxStore()
        model.handleSessionEvent(transcriptRowId: "row-1", generation: 1, event: .outboxChanged)

        XCTAssertNotNil(model.fallbackSession)
    }

    private func withScopedDiskStoreDirectory(
        _ body: @escaping @MainActor () async throws -> Void
    ) async throws {
        await DiskStore.removeAllAndWait()
        try await body()
        await DiskStore.removeAllAndWait()
    }

    private func message(id: String, type: String = "agent", content: String) throws -> Message {
        try JSONDecoder().decode(
            Message.self,
            from: Data("{\"message_id\":\"\(id)\",\"conversation_id\":\"c1\",\"sequence_id\":2,\"message_type\":\"\(type)\",\"content\":\(content)}".utf8))
    }

    private func conversation(id: String, state: String = "{\"type\":\"idle\"}") throws -> Conversation {
        try JSONDecoder().decode(
            Conversation.self,
            from: Data("{\"id\":\"\(id)\",\"slug\":\"\(id)\",\"product_conversation_id\":\"pc-1\",\"state\":\(state)}".utf8))
    }
}

private extension ProductConversationTranscriptItem {
    var debugLabel: String {
        switch self {
        case .message(let message):
            "message:\(message.message_id)"
        case .handoff(let handoff):
            "handoff:\(handoff.summaryText)"
        }
    }
}

private extension ProductConversationHandoff {
    var summaryText: String {
        switch self {
        case .completed(_, _, _, _, let summary):
            summary
        case .historical(_, _, _, let summary):
            summary
        }
    }
}
