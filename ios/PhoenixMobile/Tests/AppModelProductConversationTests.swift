import XCTest

@testable import PhoenixMobile

private func makePendingOutboxEntry(conversationId: String) -> OutboxEntry {
    OutboxEntry(
        localId: UUID().uuidString.lowercased(),
        conversationId: conversationId,
        text: "queued",
        images: [],
        status: .pending,
        acceptedByServer: false,
        createdAt: Date(),
        acceptedAt: nil,
        lastError: nil,
        attemptCount: 0)
}

private struct TestConversationSnapshot: Encodable {
    var conversation: Conversation?
    var messages: [Message]
    var lastSequenceId: Int64
    var transcriptGeneration: Int64?
    var syncedAt: Date?
}

private struct TestDiskEnvelope<Payload: Encodable>: Encodable {
    let schema_version: Int
    let payload: Payload
}

private struct TestPersistedOutboxStore: PersistedOutboxStore {
    var owners: Set<String>
    var contentsByConversationId: [String: PersistedOutboxStoreContents]

    func visibleOwnerTranscriptRowIds() -> Set<String> { owners }
    func loadContents(conversationId: String) -> PersistedOutboxStoreContents {
        contentsByConversationId[conversationId] ?? .missing
    }
}

@MainActor
final class MutableTestPersistedOutboxStore: PersistedOutboxStore {
    var owners: Set<String>
    var contentsByConversationId: [String: PersistedOutboxStoreContents]

    init(owners: Set<String>, contentsByConversationId: [String: PersistedOutboxStoreContents]) {
        self.owners = owners
        self.contentsByConversationId = contentsByConversationId
    }

    func visibleOwnerTranscriptRowIds() -> Set<String> { owners }
    func loadContents(conversationId: String) -> PersistedOutboxStoreContents {
        contentsByConversationId[conversationId] ?? .missing
    }
}

@MainActor
final class InMemoryCoordinatorIdentityStore: CoordinatorIdentityStore {
    var value: String?

    init(_ value: String? = nil) {
        self.value = value
    }

    func load() -> String? { value }
    func save(_ conversationId: String) { value = conversationId }
    func clear() { value = nil }
}

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

    private func persistReadableSnapshot(conversation: Conversation, baseDirectory: URL) {
        let phoenixDirectory = baseDirectory.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try? FileManager.default.createDirectory(at: phoenixDirectory, withIntermediateDirectories: true)
        let fileURL = phoenixDirectory.appendingPathComponent("conv-\(conversation.id).json")
        let envelope = TestDiskEnvelope(schema_version: 1, payload: TestConversationSnapshot(
            conversation: conversation,
            messages: [],
            lastSequenceId: 0,
            transcriptGeneration: 1,
            syncedAt: Date()))
        let data = try! JSONEncoder().encode(envelope)
        try! data.write(to: fileURL, options: Data.WritingOptions.atomic)
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
        let identityStore = InMemoryCoordinatorIdentityStore("coordinator-row")
        let model = AppModel(
            hasCachedSnapshot: { $0 == "coordinator-row" },
            coordinatorIdentityStore: identityStore)
        model.connectivity.setOnlineForTesting(false)

        XCTAssertEqual(model.listStore.conversations.filter(\.isCoordinator).count, 0)
        let coordinatorId = await model.openCoordinator()
        XCTAssertEqual(coordinatorId, "coordinator-row")
    }

    func testOfflineNotificationNavigationUsesCachedAggregateMember() {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)", isDirectory: true)
        let predecessor = self.conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        self.persistReadableSnapshot(conversation: predecessor, baseDirectory: baseDirectory)
        let model = AppModel(hasCachedSnapshot: { id in id == "row-1" })
        model.listStore.upsert(predecessor)
        model.listStore.upsert(self.conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        model.connectivity.setOnlineForTesting(false)

        let resolved = model.resolvedNavigationConversationId(
            aggregateId: model.listStore.aggregateId(forTranscriptRowId: "row-2"),
            latestTranscriptRowId: "row-2")

        XCTAssertEqual(resolved, "row-1")
    }
    func testOfflineHandoffNavigationUsesCachedAggregateMember() {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)", isDirectory: true)
        let predecessor = self.conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        self.persistReadableSnapshot(conversation: predecessor, baseDirectory: baseDirectory)
        let model = AppModel(hasCachedSnapshot: { id in id == "row-1" })
        model.listStore.upsert(predecessor)
        model.listStore.upsert(self.conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        model.connectivity.setOnlineForTesting(false)

        let resolved = model.resolvedNavigationConversationId(
            aggregateId: model.listStore.aggregateId(forTranscriptRowId: "row-2"),
            latestTranscriptRowId: "row-2")

        XCTAssertEqual(resolved, "row-1")
    }

    func testOfflineNavigationUsesCachedAggregateMemberWhenLatestSnapshotMissing() {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)", isDirectory: true)
        let predecessor = self.conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        self.persistReadableSnapshot(conversation: predecessor, baseDirectory: baseDirectory)
        let model = AppModel(hasCachedSnapshot: { id in id == "row-1" })
        model.listStore.upsert(predecessor)
        model.listStore.upsert(self.conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        let aggregateConversation = model.listStore.conversations.first!
        model.connectivity.setOnlineForTesting(false)

        XCTAssertEqual(model.navigationConversationId(for: aggregateConversation), "row-1")
    }

    func testOfflineNavigationUsesCachedAggregateMemberAfterRestart() {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)", isDirectory: true)
        let predecessor = self.conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        self.persistReadableSnapshot(conversation: predecessor, baseDirectory: baseDirectory)
        let first = AppModel(hasCachedSnapshot: { id in id == "row-1" })
        first.listStore.upsert(predecessor)
        first.listStore.applyExternal(
            [self.conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root")],
            startedAt: first.listStore.externalRefreshToken())

        let reloaded = AppModel(hasCachedSnapshot: { id in id == "row-1" })
        reloaded.connectivity.setOnlineForTesting(false)
        let aggregateConversation = reloaded.listStore.conversations.first!

        XCTAssertEqual(reloaded.navigationConversationId(for: aggregateConversation), "row-1")
    }
    func testColdLaunchAlreadyOnlineDrainsPersistedOutboxWithoutForegroundOrRestore() async {
        let store = TestPersistedOutboxStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])])
        let model = AppModel(persistedOutboxStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)

        let generation = model.currentPersistedOutboxDrainGenerationForTesting()
        let result = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(result, .completed(generation!))
        XCTAssertNotNil(model.existingSession(for: "row-1"))
    }

    func testOfflineLaunchWaitsAndThenDrainsOnceWhenConnectivityRestores() async {
        let store = TestPersistedOutboxStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])])
        let model = AppModel(persistedOutboxStore: store)
        model.connectivity.setOnlineForTesting(false)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let notReady = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(notReady, .notReady)
        XCTAssertNil(model.existingSession(for: "row-1"))

        model.connectivity.setOnlineForTesting(true)
        let generation = model.currentPersistedOutboxDrainGenerationForTesting()
        let result = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(result, .completed(generation!))
        XCTAssertNotNil(model.existingSession(for: "row-1"))
        let first = model.existingSession(for: "row-1")

        model.foregrounded()
        let secondGeneration = model.currentPersistedOutboxDrainGenerationForTesting()
        let secondResult = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(secondResult, .completed(secondGeneration!))
        let second = model.existingSession(for: "row-1")
        XCTAssertTrue(first === second)
    }

    func testRepeatedDrainTriggersReuseExistingDrainOwner() async {
        let store = TestPersistedOutboxStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])])
        let model = AppModel(persistedOutboxStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let firstGeneration = model.currentPersistedOutboxDrainGenerationForTesting()
        let firstResult = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(firstResult, .completed(firstGeneration!))
        let first = model.existingSession(for: "row-1")

        model.foregrounded()
        let secondGeneration = model.currentPersistedOutboxDrainGenerationForTesting()
        let secondResult = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(secondResult, .completed(secondGeneration!))
        let second = model.existingSession(for: "row-1")

        XCTAssertTrue(first === second)
    }

    func testApiLastSchedulesDrainOnceAfterApiReady() async {
        let store = TestPersistedOutboxStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])])
        let model = AppModel(persistedOutboxStore: store)
        model.configureForTesting(serverURL: "", trustSelfSigned: true)
        let notReady = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(notReady, .notReady)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let generation = model.currentPersistedOutboxDrainGenerationForTesting()
        let result = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(result, .completed(generation!))
        XCTAssertNotNil(model.existingSession(for: "row-1"))
    }

    func testReconfigurationCancelsObservationAndSchedulesNewDrainGeneration() async {
        let store = TestPersistedOutboxStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])])
        let model = AppModel(persistedOutboxStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let firstGeneration = model.currentPersistedOutboxDrainGenerationForTesting()!
        let firstDrain = await model.awaitPersistedOutboxDrainForTesting(generation: firstGeneration)
        XCTAssertEqual(firstDrain, .completed(firstGeneration))

        model.configureForTesting(serverURL: "https://example.org", trustSelfSigned: true)
        let secondGeneration = model.currentPersistedOutboxDrainGenerationForTesting()!
        XCTAssertGreaterThan(secondGeneration, firstGeneration)
        let secondDrain = await model.awaitPersistedOutboxDrainForTesting(generation: secondGeneration)
        XCTAssertEqual(secondDrain, .completed(secondGeneration))
    }

    func testSignOutClearsInjectedCoordinatorIdentityStore() async {
        let identityStore = InMemoryCoordinatorIdentityStore("coordinator-row")
        let model = AppModel(coordinatorIdentityStore: identityStore)

        await model.signOut()

        XCTAssertNil(identityStore.value)
        XCTAssertNil(model.coordinatorConversationId)
    }

    func testParallelModelInstancesDoNotInterfereThroughCoordinatorIdentity() async {
        let firstStore = InMemoryCoordinatorIdentityStore("coordinator-a")
        let secondStore = InMemoryCoordinatorIdentityStore("coordinator-b")
        let first = AppModel(coordinatorIdentityStore: firstStore)
        let second = AppModel(coordinatorIdentityStore: secondStore)

        await first.signOut()

        XCTAssertNil(firstStore.value)
        XCTAssertEqual(secondStore.value, "coordinator-b")
        XCTAssertEqual(second.coordinatorConversationId, "coordinator-b")
    }

    func testDiskPersistedOutboxStoreRejectsForeignAndMalformedEntries() {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-outbox-store-tests-\(UUID().uuidString)", isDirectory: true)
        let phoenixDirectory = baseDirectory.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try? FileManager.default.createDirectory(at: phoenixDirectory, withIntermediateDirectories: true)
        DiskStore.baseDirectory = baseDirectory
        let valid = makePendingOutboxEntry(conversationId: "row-1")
        let foreign = makePendingOutboxEntry(conversationId: "row-2")
        let validData = try! JSONEncoder().encode(TestDiskEnvelope(schema_version: Outbox.schemaVersion, payload: [valid, foreign]))
        try! validData.write(to: phoenixDirectory.appendingPathComponent("outbox-row-1.json"), options: .atomic)
        try! Data("{bad".utf8).write(to: phoenixDirectory.appendingPathComponent("outbox-.json"), options: .atomic)

        let store = DiskPersistedOutboxStore()

        XCTAssertEqual(store.visibleOwnerTranscriptRowIds(), ["row-1"])
        if case .entries(let entries) = store.loadContents(conversationId: "row-1") {
            XCTAssertEqual(entries.map(\.conversationId), ["row-1"])
        } else {
            XCTFail("expected visible entries")
        }
    }

    func testReconfigurationInvalidatesCachedDetailModel() {
        let first = AppModel()
        first.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let detailA = first.productConversationDetailModel(for: "pc-1")

        first.configureForTesting(serverURL: "https://example.org", trustSelfSigned: true)
        let detailB = first.productConversationDetailModel(for: "pc-1")

        XCTAssertFalse(detailA === detailB)
    }

    func testProductConversationDetailModelPrimesInitialTranscriptRowId() {
        let model = AppModel()
        let detail = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: PhoenixAPI(baseURL: URL(string: "https://example.com")!, password: nil, allowSelfSigned: true)!,
            connectivity: model.connectivity,
            sessionProvider: { _ in nil })

        detail.primeInitialTranscriptRowId("row-1")

        XCTAssertEqual(detail.initialTranscriptRowId, "row-1")
    }
}
