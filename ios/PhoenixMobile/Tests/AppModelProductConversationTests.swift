import XCTest

@testable import PhoenixMobile

private extension Set where Element == PersistedOutboxOwner {
    func contains(_ transcriptRowId: String) -> Bool {
        contains { $0.transcriptRowId == transcriptRowId }
    }
}

private func persistedOwners(
    _ transcriptRowIds: Set<String>,
    aggregateMembersById: [String: Set<String>]
) -> Set<PersistedOutboxOwner> {
    Set(transcriptRowIds.map { transcriptRowId in
        PersistedOutboxOwner(
            transcriptRowId: transcriptRowId,
            aggregateAuthority: aggregateMembersById.first(where: {
                $0.value.contains(transcriptRowId)
            })?.key)
    })
}

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

private struct TestConversationSnapshot: Codable {
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

private final class InMemoryCredentialStore: CredentialStore {
    enum Fault: Error {
        case saveRecordFailed
    }

    var record: AppModel.CredentialRecord?
    var failNextSave = false
    var legacyPassword: String?
    private(set) var deletedAccounts: [String] = []

    func loadLegacyPassword(account: String) -> String? { legacyPassword }
    func loadRecord(account: String) -> AppModel.CredentialRecord? { record }
    func saveRecord(_ record: AppModel.CredentialRecord, account: String) throws {
        if failNextSave {
            failNextSave = false
            throw Fault.saveRecordFailed
        }
        self.record = record
    }
    func deleteRecord(account: String) {
        if account == "server-password" {
            legacyPassword = nil
        } else {
            record = nil
        }
        deletedAccounts.append(account)
    }
}


private func testProductConversationSnapshot() -> ProductConversationSnapshot {
    ProductConversationSnapshot(
        product_conversation_id: "pc-1",
        close: nil,
        canonical_route: "/product-conversations/pc-1",
        requested_transcript_row_id: "row-2",
        canonical_root: .init(transcript_row_id: "row-1", slug: "root", title: "Root"),
        ordinary_lifecycle: .open,
        latest_transcript_row_id: "row-2",
        writable_transcript_row_id: "row-2",
        updated_at: "2025-01-02T03:04:05Z",
        presentation: .state(displayName: "Root", presentationMode: "working"),
        work_identity: nil,
        source: nil,
        chain_qa_compatibility: nil,
        segments: [
            .init(segment_ordinal: 0, transcript_row_id: "row-1", slug: "root", title: "Root", messages: [], handoff: .historical(predecessorTranscriptRowId: "row-1", successorTranscriptRowId: "row-2", continuationMessageId: "m-cont", summary: "summary")),
            .init(segment_ordinal: 1, transcript_row_id: "row-2", slug: "next", title: "Next", messages: [], handoff: nil)
        ],
        before: nil,
        has_older: false)
}

private func testSingleSegmentProductConversationSnapshot() -> ProductConversationSnapshot {
    var snapshot = testProductConversationSnapshot()
    snapshot.requested_transcript_row_id = "row-1"
    snapshot.latest_transcript_row_id = "row-1"
    snapshot.writable_transcript_row_id = "row-1"
    snapshot.segments = [
        .init(
            segment_ordinal: 0,
            transcript_row_id: "row-1",
            slug: "root",
            title: "Root",
            messages: [],
            handoff: nil)
    ]
    return snapshot
}

@MainActor
private final class InMemoryOutboxStore {
    private var entriesByConversationId: [String: [OutboxEntry]]
    private var owners: Set<String>
    private var revisionsByConversationId: [String: Int]
    private var writableByConversationId: [String: Bool]

    init(contentsByConversationId: [String: PersistedOutboxStoreContents], owners: Set<String> = []) {
        self.entriesByConversationId = contentsByConversationId.reduce(into: [:]) { result, pair in
            if case .entries(let entries) = pair.value {
                result[pair.key] = entries
            }
        }
        self.owners = owners
        for owner in owners where self.entriesByConversationId[owner] == nil {
            self.entriesByConversationId[owner] = []
        }
        self.revisionsByConversationId = [:]
        self.writableByConversationId = [:]
    }

    var ownerTranscriptRowIds: Set<String> {
        owners.filter { writableByConversationId[$0, default: true] }
    }

    func inspect(conversationId: String, aggregateAuthority: String? = nil) -> OutboxStoreInspection {
        guard writableByConversationId[conversationId, default: true],
              owners.contains(conversationId)
        else {
            return OutboxStoreInspection(conversationId: conversationId, state: .missing)
        }
        let entries = entriesByConversationId[conversationId] ?? []
        return OutboxStoreInspection(
            conversationId: conversationId,
            state: .accessible(
                scope: PersistenceScopeIdentity(
                    serverURL: "https://example.com",
                    credentialGeneration: "test-default"),
                aggregateAuthority: aggregateAuthority,
                entries: entries))
    }

    func handle(
        for conversationId: String,
        aggregateAuthority: String?,
        scope: PersistenceScopeIdentity
    ) -> OutboxPersistenceHandle {
        OutboxPersistenceHandle(
            inspect: { [weak self] requestedConversationId in
                guard let self else {
                    return OutboxStoreInspection(conversationId: requestedConversationId, state: .missing)
                }
                guard self.writableByConversationId[requestedConversationId, default: true],
                      self.owners.contains(requestedConversationId)
                else {
                    return OutboxStoreInspection(conversationId: requestedConversationId, state: .missing)
                }
                return OutboxStoreInspection(
                    conversationId: requestedConversationId,
                    state: .accessible(
                        scope: scope,
                        aggregateAuthority: aggregateAuthority,
                        entries: self.entriesByConversationId[requestedConversationId] ?? []))
            },
            reserveRevision: { [weak self] in
                guard let self else { return 0 }
                let next = self.revisionsByConversationId[conversationId, default: 0] + 1
                self.revisionsByConversationId[conversationId] = next
                return next
            },
            save: { [weak self] envelope, revision in
                guard let self else { return false }
                guard self.writableByConversationId[conversationId, default: true] else { return false }
                guard self.revisionsByConversationId[conversationId, default: 0] == revision else { return false }
                self.entriesByConversationId[conversationId] = envelope.entries
                if envelope.entries.isEmpty {
                    self.owners.remove(conversationId)
                } else {
                    self.owners.insert(conversationId)
                }
                return true
            },
            remove: { [weak self] revision in
                guard let self else { return }
                guard self.revisionsByConversationId[conversationId, default: 0] == revision else { return }
                self.entriesByConversationId.removeValue(forKey: conversationId)
                self.owners.remove(conversationId)
                self.writableByConversationId[conversationId] = false
            })
    }

    func removePersistedConversationState(conversationId: String) async {
        let revision = revisionsByConversationId[conversationId, default: 0] + 1
        revisionsByConversationId[conversationId] = revision
        entriesByConversationId.removeValue(forKey: conversationId)
        owners.remove(conversationId)
        writableByConversationId[conversationId] = false
    }
}

private final class TestConversationPersistenceStore: ConversationPersistenceStore {
    var listPersistenceContext: VersionedDiskContext? { nil }
    var persistenceScope: PersistenceScopeIdentity? { nil }
    func persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity) -> Set<PersistedOutboxOwner> { persistedOwners(outboxStore.ownerTranscriptRowIds, aggregateMembersById: aggregateMembersById) }
    let baseDirectory: URL
    var snapshotsByConversationId: Set<String> = []
    var aggregateMembersById: [String: Set<String>] = [:]
    private let outboxStore: InMemoryOutboxStore

    init(baseDirectory: URL = FileManager.default.temporaryDirectory.appendingPathComponent("phoenix-test-store-\(UUID().uuidString)"), owners: Set<String> = [], contentsByConversationId: [String: PersistedOutboxStoreContents], snapshotsByConversationId: Set<String> = [], aggregateMembersById: [String: Set<String>] = [:]) {
        self.baseDirectory = baseDirectory
        self.snapshotsByConversationId = snapshotsByConversationId
        self.aggregateMembersById = aggregateMembersById
        self.outboxStore = InMemoryOutboxStore(contentsByConversationId: contentsByConversationId, owners: owners)
    }

    func pendingOutboxOwners(scope: PersistenceScopeIdentity) async -> Set<PersistedOutboxOwner> {
        let ids = Set(outboxStore.ownerTranscriptRowIds.filter { conversationId in
            outboxStore.inspect(conversationId: conversationId).hasPendingSendableEntries
        })
        return persistedOwners(ids, aggregateMembersById: aggregateMembersById)
    }

    func hasCachedSnapshot(conversationId: String) -> Bool { snapshotsByConversationId.contains(conversationId) }
    func hasAuthoritativeCachedSnapshot(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) -> Bool { snapshotsByConversationId.contains(conversationId) }
    func inspectOutbox(conversationId: String) -> OutboxStoreInspection {
        outboxStore.inspect(
            conversationId: conversationId,
            aggregateAuthority: aggregateMembersById.first(where: { $0.value.contains(conversationId) })?.key)
    }
    func outboxPersistence(conversationId: String, aggregateAuthority: String?, scope: PersistenceScopeIdentity) -> OutboxPersistenceHandle { outboxStore.handle(for: conversationId, aggregateAuthority: aggregateAuthority, scope: scope) }
    func snapshotPersistence(conversationId: String) -> VersionedDiskWriter {
        let destination = baseDirectory.appendingPathComponent("PhoenixMobile", isDirectory: true)
            .appendingPathComponent("conv-\(conversationId)")
            .appendingPathExtension("json")
        return DiskStore.versionedContext(baseDirectory: FileManager.default.temporaryDirectory).writer(destinationURL: destination, version: ConversationSession.snapshotSchemaVersion)
    }
    func removePersistedConversationState(conversationId: String) async { await outboxStore.removePersistedConversationState(conversationId: conversationId) }
    func removeAuthoritativePersistedConversationState(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) async -> Bool {
        await removePersistedConversationState(conversationId: conversationId)
        return true
    }
    func replaceHardDeleteFence(expected: PersistedHardDeleteFence?, replacement: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome {
        expected == nil ? .replaced : .expectationMismatch
    }
    func hardDeleteFences(persistenceScope: PersistenceScopeIdentity) -> HardDeleteFenceLoadResult { .accessible([]) }
    func retireHardDeleteFence(expected: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome { .replaced }
    func removeAllPersistedConversationState() async {
        for conversationId in outboxStore.ownerTranscriptRowIds {
            await outboxStore.removePersistedConversationState(conversationId: conversationId)
        }
        snapshotsByConversationId.removeAll()
        aggregateMembersById.removeAll()
    }
    func persistedConversationIds(aggregateId: String, scope: PersistenceScopeIdentity) -> Set<String> { aggregateMembersById[aggregateId] ?? [] }
    func resetConversationListCache() async {}
}

private final class SendProbe {
    private let lock = NSLock()
    private var chatPostPathsStorage: [String] = []
    private var aggregateGetPathsStorage: [String] = []
    private var listGetPathsStorage: [String] = []
    private var archivePostPathsStorage: [String] = []

    func record(_ request: URLRequest) {
        let path = request.url!.path
        lock.lock()
        defer { lock.unlock() }
        if request.httpMethod == "POST", path.contains("/chat") {
            chatPostPathsStorage.append(path)
        }
        if request.httpMethod == "GET", path.contains("/api/product-conversations/") {
            aggregateGetPathsStorage.append(path)
        }
        if request.httpMethod == "GET", path == "/api/product-conversations" {
            listGetPathsStorage.append(path)
        }
        if request.httpMethod == "POST", path.contains("/archive") {
            archivePostPathsStorage.append(path)
        }
    }

    var chatPostPaths: [String] {
        lock.lock()
        defer { lock.unlock() }
        return chatPostPathsStorage
    }

    var archivePostPaths: [String] {
        lock.lock()
        defer { lock.unlock() }
        return archivePostPathsStorage
    }

    var aggregateGetPaths: [String] {
        lock.lock()
        defer { lock.unlock() }
        return aggregateGetPathsStorage
    }

    var listGetPaths: [String] {
        lock.lock()
        defer { lock.unlock() }
        return listGetPathsStorage
    }
}

private actor AsyncCandidateGate {
    private var enteredCount = 0
    private var released = false
    private var entryWaiters: [CheckedContinuation<Void, Never>] = []
    private var releaseWaiters: [CheckedContinuation<Void, Never>] = []

    func waitForEntry(count: Int = 1) async {
        if enteredCount >= count { return }
        await withCheckedContinuation { entryWaiters.append($0) }
    }

    func awaitRelease() async {
        if released { return }
        await withCheckedContinuation { releaseWaiters.append($0) }
    }

    func markEntered() {
        enteredCount += 1
        let waiters = entryWaiters
        entryWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }

    func release() {
        released = true
        let waiters = releaseWaiters
        releaseWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }
}

private actor DrainBlocker {
    private var entered = false
    private var entryWaiters: [CheckedContinuation<Void, Never>] = []
    private var releaseWaiters: [CheckedContinuation<Void, Never>] = []
    private var released = false

    func waitForEntry() async {
        if entered { return }
        await withCheckedContinuation { entryWaiters.append($0) }
    }

    func block() async {
        entered = true
        let waiters = entryWaiters
        entryWaiters.removeAll()
        waiters.forEach { $0.resume() }
        if released { return }
        await withCheckedContinuation { releaseWaiters.append($0) }
    }

    func release() async {
        released = true
        let waiters = releaseWaiters
        releaseWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }
}

private actor CompletionProbe {
    private var completed = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func markCompleted() {
        completed = true
        let current = waiters
        waiters.removeAll()
        current.forEach { $0.resume() }
    }

    func wait() async {
        if completed { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    func isCompleted() -> Bool { completed }
}

@MainActor
final class ResettableConversationPersistenceStore: ConversationPersistenceStore {
    private let wrapped: DiskConversationPersistenceStore
    var listPersistenceContext: VersionedDiskContext? { wrapped.listPersistenceContext }
    var persistenceScope: PersistenceScopeIdentity? { wrapped.persistenceScope }
    private let resetBlocker: DrainBlocker

    fileprivate init(baseDirectory: URL, context: VersionedDiskContext, resetBlocker: DrainBlocker) {
        self.wrapped = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        self.resetBlocker = resetBlocker
    }

    func pendingOutboxOwners(scope: PersistenceScopeIdentity) async -> Set<PersistedOutboxOwner> {
        await wrapped.pendingOutboxOwners(scope: scope)
    }

    func persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity) -> Set<PersistedOutboxOwner> {
        wrapped.persistedOutboxOwnersSnapshot(scope: scope)
    }

    func hasCachedSnapshot(conversationId: String) -> Bool {
        wrapped.hasCachedSnapshot(conversationId: conversationId)
    }

    func hasAuthoritativeCachedSnapshot(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String
    ) -> Bool {
        wrapped.hasAuthoritativeCachedSnapshot(
            conversationId: conversationId,
            configurationIdentity: configurationIdentity,
            aggregateAuthority: aggregateAuthority)
    }

    func inspectOutbox(conversationId: String) -> OutboxStoreInspection {
        wrapped.inspectOutbox(conversationId: conversationId)
    }

    func outboxPersistence(conversationId: String, aggregateAuthority: String?, scope: PersistenceScopeIdentity) -> OutboxPersistenceHandle {
        wrapped.outboxPersistence(
            conversationId: conversationId,
            aggregateAuthority: aggregateAuthority,
            scope: scope)
    }

    func snapshotPersistence(conversationId: String) -> VersionedDiskWriter {
        wrapped.snapshotPersistence(conversationId: conversationId)
    }

    func persistedConversationIds(aggregateId: String, scope: PersistenceScopeIdentity) -> Set<String> {
        wrapped.persistedConversationIds(aggregateId: aggregateId, scope: scope)
    }

    func resetConversationListCache() async {
        await resetBlocker.block()
        await wrapped.resetConversationListCache()
    }

    func removePersistedConversationState(conversationId: String) async {
        await wrapped.removePersistedConversationState(conversationId: conversationId)
    }

    func removeAuthoritativePersistedConversationState(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) async -> Bool {
        await removePersistedConversationState(conversationId: conversationId)
        return true
    }
    func replaceHardDeleteFence(expected: PersistedHardDeleteFence?, replacement: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome {
        expected == nil ? .replaced : .expectationMismatch
    }
    func hardDeleteFences(persistenceScope: PersistenceScopeIdentity) -> HardDeleteFenceLoadResult { .accessible([]) }
    func retireHardDeleteFence(expected: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome { .replaced }
    func removeAllPersistedConversationState() async {
        await resetBlocker.block()
        await wrapped.removeAllPersistedConversationState()
    }
}

@MainActor
final class HardDeleteGatedConversationPersistenceStore: ConversationPersistenceStore {
    var listPersistenceContext: VersionedDiskContext? { nil }
    var persistenceScope: PersistenceScopeIdentity? { nil }
    private let outboxStore: InMemoryOutboxStore
    var aggregateMembersById: [String: Set<String>]
    private let blocker: DrainBlocker
    private let removedProbe: CompletionProbe

    fileprivate init(
        owners: Set<String>,
        contentsByConversationId: [String: PersistedOutboxStoreContents],
        aggregateMembersById: [String: Set<String>],
        blocker: DrainBlocker,
        removedProbe: CompletionProbe
    ) {
        self.outboxStore = InMemoryOutboxStore(contentsByConversationId: contentsByConversationId, owners: owners)
        self.aggregateMembersById = aggregateMembersById
        self.blocker = blocker
        self.removedProbe = removedProbe
    }

    func pendingOutboxOwners(scope: PersistenceScopeIdentity) async -> Set<PersistedOutboxOwner> { persistedOwners(outboxStore.ownerTranscriptRowIds, aggregateMembersById: [:]) }
    func persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity) -> Set<PersistedOutboxOwner> { persistedOwners(outboxStore.ownerTranscriptRowIds, aggregateMembersById: aggregateMembersById) }
    func hasCachedSnapshot(conversationId: String) -> Bool { false }
    func hasAuthoritativeCachedSnapshot(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) -> Bool { false }
    func inspectOutbox(conversationId: String) -> OutboxStoreInspection {
        outboxStore.inspect(
            conversationId: conversationId,
            aggregateAuthority: aggregateMembersById.first(where: { $0.value.contains(conversationId) })?.key)
    }
    func outboxPersistence(conversationId: String, aggregateAuthority: String?, scope: PersistenceScopeIdentity) -> OutboxPersistenceHandle { outboxStore.handle(for: conversationId, aggregateAuthority: aggregateAuthority, scope: scope) }
    func snapshotPersistence(conversationId: String) -> VersionedDiskWriter {
        DiskStore.versionedContext(baseDirectory: FileManager.default.temporaryDirectory)
            .writer(destinationURL: FileManager.default.temporaryDirectory.appendingPathComponent("hard-delete-\(conversationId).json"), version: ConversationSession.snapshotSchemaVersion)
    }
    func persistedConversationIds(aggregateId: String, scope: PersistenceScopeIdentity) -> Set<String> { aggregateMembersById[aggregateId] ?? [] }
    func resetConversationListCache() async {}
    func removePersistedConversationState(conversationId: String) async {
        await blocker.block()
        await outboxStore.removePersistedConversationState(conversationId: conversationId)
        await removedProbe.markCompleted()
    }
    func removeAuthoritativePersistedConversationState(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) async -> Bool {
        await removePersistedConversationState(conversationId: conversationId)
        return true
    }
    func replaceHardDeleteFence(expected: PersistedHardDeleteFence?, replacement: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome {
        expected == nil ? .replaced : .expectationMismatch
    }
    func hardDeleteFences(persistenceScope: PersistenceScopeIdentity) -> HardDeleteFenceLoadResult { .accessible([]) }
    func retireHardDeleteFence(expected: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome { .replaced }
    func removeAllPersistedConversationState() async {}
}

@MainActor
final class GatedConversationPersistenceStore: ConversationPersistenceStore {
    var listPersistenceContext: VersionedDiskContext? { nil }
    var persistenceScope: PersistenceScopeIdentity? { nil }
    func persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity) -> Set<PersistedOutboxOwner> { persistedOwners(outboxStore.ownerTranscriptRowIds, aggregateMembersById: aggregateMembersById) }
    var snapshotsByConversationId: Set<String>
    var aggregateMembersById: [String: Set<String>]
    private let outboxStore: InMemoryOutboxStore
    fileprivate let gate: AsyncCandidateGate

    fileprivate init(
        owners: Set<String> = [],
        contentsByConversationId: [String: PersistedOutboxStoreContents],
        snapshotsByConversationId: Set<String> = [],
        aggregateMembersById: [String: Set<String>] = [:],
        gate: AsyncCandidateGate
    ) {
        self.snapshotsByConversationId = snapshotsByConversationId
        self.aggregateMembersById = aggregateMembersById
        self.outboxStore = InMemoryOutboxStore(contentsByConversationId: contentsByConversationId, owners: owners)
        self.gate = gate
    }

    func pendingOutboxOwners(scope: PersistenceScopeIdentity) async -> Set<PersistedOutboxOwner> {
        await gate.markEntered()
        await gate.awaitRelease()
        let ids = Set(outboxStore.ownerTranscriptRowIds.filter { conversationId in
            outboxStore.inspect(conversationId: conversationId).hasPendingSendableEntries
        })
        return persistedOwners(ids, aggregateMembersById: aggregateMembersById)
    }

    func hasCachedSnapshot(conversationId: String) -> Bool { snapshotsByConversationId.contains(conversationId) }
    func hasAuthoritativeCachedSnapshot(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) -> Bool { snapshotsByConversationId.contains(conversationId) }
    func inspectOutbox(conversationId: String) -> OutboxStoreInspection {
        outboxStore.inspect(
            conversationId: conversationId,
            aggregateAuthority: aggregateMembersById.first(where: { $0.value.contains(conversationId) })?.key)
    }
    func outboxPersistence(conversationId: String, aggregateAuthority: String?, scope: PersistenceScopeIdentity) -> OutboxPersistenceHandle { outboxStore.handle(for: conversationId, aggregateAuthority: aggregateAuthority, scope: scope) }
    func snapshotPersistence(conversationId: String) -> VersionedDiskWriter {
        let destination = FileManager.default.temporaryDirectory
            .appendingPathComponent("PhoenixMobile", isDirectory: true)
            .appendingPathComponent("conv-\(conversationId)")
            .appendingPathExtension("json")
        return DiskStore.versionedContext(baseDirectory: FileManager.default.temporaryDirectory).writer(destinationURL: destination, version: ConversationSession.snapshotSchemaVersion)
    }
    func removePersistedConversationState(conversationId: String) async {
        await outboxStore.removePersistedConversationState(conversationId: conversationId)
        snapshotsByConversationId.remove(conversationId)
        for aggregateId in aggregateMembersById.keys {
            aggregateMembersById[aggregateId]?.remove(conversationId)
        }
    }
    func removeAuthoritativePersistedConversationState(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) async -> Bool {
        await removePersistedConversationState(conversationId: conversationId)
        return true
    }
    func replaceHardDeleteFence(expected: PersistedHardDeleteFence?, replacement: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome {
        expected == nil ? .replaced : .expectationMismatch
    }
    func hardDeleteFences(persistenceScope: PersistenceScopeIdentity) -> HardDeleteFenceLoadResult { .accessible([]) }
    func retireHardDeleteFence(expected: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome { .replaced }
    func removeAllPersistedConversationState() async {
        for conversationId in outboxStore.ownerTranscriptRowIds {
            await outboxStore.removePersistedConversationState(conversationId: conversationId)
        }
        snapshotsByConversationId.removeAll()
        aggregateMembersById.removeAll()
    }
    func persistedConversationIds(aggregateId: String, scope: PersistenceScopeIdentity) -> Set<String> { aggregateMembersById[aggregateId] ?? [] }
    func resetConversationListCache() async {}
}

@MainActor
final class MutableTestConversationPersistenceStore: ConversationPersistenceStore {
    var listPersistenceContext: VersionedDiskContext? { nil }
    var persistenceScope: PersistenceScopeIdentity? { nil }
    func persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity) -> Set<PersistedOutboxOwner> { persistedOwners(outboxStore.ownerTranscriptRowIds, aggregateMembersById: aggregateMembersById) }
    var snapshotsByConversationId: Set<String>
    var aggregateMembersById: [String: Set<String>]
    var persistedMemberDiscoveryOverride: PersistedMemberDiscovery?
    var onPendingOutboxOwnerTranscriptRowIds: (() async -> Set<String>)?
    fileprivate var persistedMemberDiscoveryGate: AsyncCandidateGate?
    var hardDeleteFenceLoadResult: HardDeleteFenceLoadResult = .accessible([])
    var persistHardDeleteFenceResult = true
    private(set) var persistedHardDeleteFences: [PersistedHardDeleteFence] = []
    private(set) var persistedHardDeleteFenceHistory: [PersistedHardDeleteFence] = []
    fileprivate var persistHardDeleteFenceGate: AsyncCandidateGate?
    private(set) var hardDeleteFencePersistAttemptCount = 0

    private var deliveryPreparationGate: (entered: AsyncCandidateGate, completed: AsyncCandidateGate)?
    private var removeAllGate: AsyncCandidateGate?
    private(set) var hardDeleteFenceLoadCount = 0
    private(set) var pendingOutboxDiscoveryCount = 0
    private let outboxStore: InMemoryOutboxStore

    init(owners: Set<String> = [], contentsByConversationId: [String: PersistedOutboxStoreContents], snapshotsByConversationId: Set<String> = [], aggregateMembersById: [String: Set<String>] = [:]) {
        self.snapshotsByConversationId = snapshotsByConversationId
        self.aggregateMembersById = aggregateMembersById
        self.outboxStore = InMemoryOutboxStore(contentsByConversationId: contentsByConversationId, owners: owners)
    }

    func persistedMemberDiscovery(
        aggregateId: String,
        scope: PersistenceScopeIdentity
    ) async -> PersistedMemberDiscovery {
        if let persistedMemberDiscoveryGate {
            await persistedMemberDiscoveryGate.markEntered()
            await persistedMemberDiscoveryGate.awaitRelease()
        }
        if let persistedMemberDiscoveryOverride { return persistedMemberDiscoveryOverride }
        return await superPersistedMemberDiscovery(aggregateId: aggregateId, scope: scope)
    }

    private func superPersistedMemberDiscovery(
        aggregateId: String,
        scope: PersistenceScopeIdentity
    ) async -> PersistedMemberDiscovery {
        let members = persistedConversationIds(aggregateId: aggregateId, scope: scope)
        let owners = await pendingOutboxOwners(scope: scope)
        return .init(
            currentAuthorityMemberIds: members,
            persistedOutboxOwnerIds: Set(owners.compactMap {
                $0.aggregateAuthority == aggregateId ? $0.transcriptRowId : nil
            }))
    }

    func pendingOutboxOwners(scope: PersistenceScopeIdentity) async -> Set<PersistedOutboxOwner> {
        pendingOutboxDiscoveryCount += 1
        let ids: Set<String>
        if let onPendingOutboxOwnerTranscriptRowIds {
            ids = await onPendingOutboxOwnerTranscriptRowIds()
        } else {
            ids = Set(outboxStore.ownerTranscriptRowIds.filter { conversationId in
                outboxStore.inspect(conversationId: conversationId).hasPendingSendableEntries
            })
        }
        return persistedOwners(ids, aggregateMembersById: aggregateMembersById)
    }
    fileprivate func suspendNextDeliveryPreparation(
        using gate: AsyncCandidateGate,
        completed: AsyncCandidateGate
    ) {
        deliveryPreparationGate = (gate, completed)
    }
    fileprivate func suspendRemoveAll(using gate: AsyncCandidateGate) {
        removeAllGate = gate
    }
    func hasCachedSnapshot(conversationId: String) -> Bool { snapshotsByConversationId.contains(conversationId) }
    func hasAuthoritativeCachedSnapshot(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) -> Bool { snapshotsByConversationId.contains(conversationId) }
    func inspectOutbox(conversationId: String) -> OutboxStoreInspection {
        outboxStore.inspect(
            conversationId: conversationId,
            aggregateAuthority: aggregateMembersById.first(where: { $0.value.contains(conversationId) })?.key)
    }
    func outboxPersistence(conversationId: String, aggregateAuthority: String?, scope: PersistenceScopeIdentity) -> OutboxPersistenceHandle {
        let underlying = outboxStore.handle(
            for: conversationId, aggregateAuthority: aggregateAuthority, scope: scope)
        return OutboxPersistenceHandle(
            inspect: { underlying.inspect(conversationId: $0) },
            reserveRevision: { underlying.reserveRevision() },
            save: { [weak self] envelope, revision in
                if let suspension = self?.deliveryPreparationGate {
                    self?.deliveryPreparationGate = nil
                    await suspension.entered.markEntered()
                    await suspension.entered.awaitRelease()
                    let saved = await underlying.save(envelope, revision: revision)
                    await suspension.completed.markEntered()
                    return saved
                }
                return await underlying.save(envelope, revision: revision)
            },
            remove: { revision in await underlying.remove(revision: revision) })
    }
    func snapshotPersistence(conversationId: String) -> VersionedDiskWriter {
        let destination = FileManager.default.temporaryDirectory
            .appendingPathComponent("PhoenixMobile", isDirectory: true)
            .appendingPathComponent("conv-\(conversationId)")
            .appendingPathExtension("json")
        return DiskStore.versionedContext(baseDirectory: FileManager.default.temporaryDirectory).writer(destinationURL: destination, version: ConversationSession.snapshotSchemaVersion)
    }
    func removePersistedConversationState(conversationId: String) async {
        await outboxStore.removePersistedConversationState(conversationId: conversationId)
        snapshotsByConversationId.remove(conversationId)
        for aggregateId in aggregateMembersById.keys {
            aggregateMembersById[aggregateId]?.remove(conversationId)
        }
    }
    func removeAuthoritativePersistedConversationState(conversationId: String, configurationIdentity: APIConfigurationIdentity, aggregateAuthority: String) async -> Bool {
        await removePersistedConversationState(conversationId: conversationId)
        return true
    }
    func replaceHardDeleteFence(expected: PersistedHardDeleteFence?, replacement: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome {
        hardDeleteFencePersistAttemptCount += 1
        if let persistHardDeleteFenceGate {
            await persistHardDeleteFenceGate.markEntered()
            await persistHardDeleteFenceGate.awaitRelease()
        }
        let current = persistedHardDeleteFences.first { $0.storageName == replacement.storageName }
        guard current == expected else { return .expectationMismatch }
        guard persistHardDeleteFenceResult else { return .persistenceFailed }
        persistedHardDeleteFences.removeAll { $0.storageName == replacement.storageName }
        persistedHardDeleteFences.append(replacement)
        persistedHardDeleteFenceHistory.append(replacement)
        return .replaced
    }
    func hardDeleteFences(persistenceScope: PersistenceScopeIdentity) -> HardDeleteFenceLoadResult {
        hardDeleteFenceLoadCount += 1
        switch hardDeleteFenceLoadResult {
        case .inaccessible:
            return .inaccessible
        case .accessible:
            return .accessible(persistedHardDeleteFences.filter {
                $0.persistenceScope == persistenceScope
            })
        }
    }
    func retireHardDeleteFence(expected: PersistedHardDeleteFence) async -> HardDeleteFenceMutationOutcome {
        guard persistedHardDeleteFences.contains(expected) else { return .expectationMismatch }
        persistedHardDeleteFences.removeAll { $0 == expected }
        return .replaced
    }
    func removeAllPersistedConversationState() async {
        if let gate = removeAllGate {
            removeAllGate = nil
            await gate.markEntered()
            await gate.awaitRelease()
        }
        for conversationId in outboxStore.ownerTranscriptRowIds {
            await outboxStore.removePersistedConversationState(conversationId: conversationId)
        }
        snapshotsByConversationId.removeAll()
        aggregateMembersById.removeAll()
    }
    func persistedConversationIds(aggregateId: String, scope: PersistenceScopeIdentity) -> Set<String> { aggregateMembersById[aggregateId] ?? [] }
    func resetConversationListCache() async {}
}

@MainActor
final class InMemoryCoordinatorIdentityStore: CoordinatorIdentityStore {
    var receiptsByPersistenceScope: [PersistenceScopeIdentity: CoordinatorIdentityReceipt]

    init(_ value: String? = nil, configurationIdentity: APIConfigurationIdentity = APIConfigurationIdentity(serverURL: "https://example.com", credentialGeneration: "test-default", trustSelfSigned: true)) {
        if let value {
            receiptsByPersistenceScope = [configurationIdentity.persistenceScope: CoordinatorIdentityReceipt(persistenceScope: configurationIdentity.persistenceScope, conversationId: value)]
        } else {
            receiptsByPersistenceScope = [:]
        }
    }

    var value: String? {
        receiptsByPersistenceScope.values.first?.conversationId
    }

    func load(persistenceScope: PersistenceScopeIdentity) -> CoordinatorIdentityReceipt? {
        receiptsByPersistenceScope[persistenceScope]
    }

    func save(_ receipt: CoordinatorIdentityReceipt) {
        receiptsByPersistenceScope[receipt.persistenceScope] = receipt
    }

    func clear(persistenceScope: PersistenceScopeIdentity) {
        receiptsByPersistenceScope.removeValue(forKey: persistenceScope)
    }

    func clearAll() {
        receiptsByPersistenceScope.removeAll()
    }

    func resetConversationListCache() async {}
}

@MainActor
final class AppModelProductConversationTests: XCTestCase {
    private let defaultConfigurationIdentity = APIConfigurationIdentity(
        serverURL: "https://example.com",
        credentialGeneration: "test-default",
        trustSelfSigned: true)
    private let credentialStore = InMemoryCredentialStore()
    private var defaultPersistenceScope: PersistenceScopeIdentity {
        defaultConfigurationIdentity.persistenceScope
    }

    override class func setUp() {
        super.setUp()
        URLProtocol.registerClass(TestURLProtocol.self)
    }

    override class func tearDown() {
        URLProtocol.unregisterClass(TestURLProtocol.self)
        super.tearDown()
    }

    private func makeModel(
        hasCachedSnapshot: ((String) -> Bool)? = nil,
        conversationPersistenceStore: ConversationPersistenceStore? = nil,
        coordinatorIdentityStore: CoordinatorIdentityStore? = nil
    ) -> AppModel {
        AppModel(
            hasCachedSnapshot: hasCachedSnapshot,
            conversationPersistenceStore: conversationPersistenceStore,
            coordinatorIdentityStore: coordinatorIdentityStore,
            credentialStore: credentialStore)
    }

    private func inspectedEntries(
        _ store: some ConversationPersistenceStore,
        conversationId: String
    ) -> [OutboxEntry]? {
        let inspection = store.inspectOutbox(conversationId: conversationId)
        guard case .accessible(_, _, let entries) = inspection.state else { return nil }
        return entries
    }

    private func conversation(
        id: String,
        aggregateId: String? = nil,
        slug: String? = nil,
        title: String? = nil,
        taskTitle: String? = nil,
        archived: Bool? = nil,
        mode: String? = nil,
        state: JSONValue? = nil,
        updatedAt: String? = nil,
        runtimeRole: String? = nil,
        closeAction: ProductConversationCloseAction? = nil,
        chainRootId: String? = nil
    ) -> Conversation {
        Conversation(
            id: id,
            product_conversation_id: aggregateId,
            chain_root_id: chainRootId,
            slug: slug,
            title: title,
            model: nil,
            cwd: nil,
            created_at: nil,
            updated_at: updatedAt,
            message_count: nil,
            state: state,
            state_updated_at: nil,
            branch_name: nil,
            task_title: taskTitle,
            archived: archived,
            product_close_action: closeAction,
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

    private func isolatedDiskDirectory() -> URL {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)", isDirectory: true)
        DiskStore.baseDirectory = baseDirectory
        return baseDirectory
    }

    func testInMemoryOutboxStoreDiscoversEmptyOwners() {
        let store = InMemoryOutboxStore(contentsByConversationId: [:], owners: ["row-1"])
        XCTAssertEqual(store.ownerTranscriptRowIds, ["row-1"])
        if case .accessible(_, _, let entries) = store.inspect(conversationId: "row-1").state {
            XCTAssertTrue(entries.isEmpty)
        } else {
            XCTFail("expected empty owned outbox file")
        }
        if case .missing = store.inspect(conversationId: "row-2").state {
        } else {
            XCTFail("unexpected absent owner contents")
        }
    }

    @MainActor
    func testSchemaV1OutboxDoesNotMintCurrentTenantDrainAuthority() async {
        let baseDirectory = isolatedDiskDirectory()
        let entry = makePendingOutboxEntry(conversationId: "row-legacy")
        XCTAssertTrue(DiskStore.saveVersioned([entry], name: "outbox-row-legacy", version: 1))
        let source = DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
            .appendingPathComponent("outbox-row-legacy")
            .appendingPathExtension("json")
        let bytesBeforeInspection = try! Data(contentsOf: source)
        let store = DiskConversationPersistenceStore(
            baseDirectory: baseDirectory,
            context: DiskStore.versionedContext(baseDirectory: baseDirectory))
        let owners = await store.pendingOutboxOwners(scope: defaultPersistenceScope)

        XCTAssertTrue(owners.isEmpty)
        XCTAssertEqual(try! Data(contentsOf: source), bytesBeforeInspection)
        if case .accessible(let scope, let aggregateAuthority, _) = store.inspectOutbox(conversationId: "row-legacy").state {
            XCTAssertNil(scope)
            XCTAssertNil(aggregateAuthority)
        } else {
            XCTFail("expected legacy queue to stay inspectable but authority-free")
        }
    }

    @MainActor
    func testFailedLegacyCredentialMigrationLeavesAppUnconfigured() {
        let credentials = InMemoryCredentialStore()
        credentials.legacyPassword = "legacy-secret"
        credentials.failNextSave = true
        UserDefaults.standard.set("https://example.com", forKey: "phoenix.serverURL")
        defer { UserDefaults.standard.removeObject(forKey: "phoenix.serverURL") }

        let model = AppModel(
            conversationPersistenceStore: MutableTestConversationPersistenceStore(contentsByConversationId: [:]),
            credentialStore: credentials)

        XCTAssertFalse(model.isConfigured)
        XCTAssertEqual(model.password, "")
        XCTAssertEqual(model.credentialGeneration, "")
        XCTAssertNil(credentials.record)
        XCTAssertEqual(credentials.legacyPassword, "legacy-secret")
    }

    private func makeHTTPAPI(
        probe: SendProbe,
        host: String = "phoenix.invalid",
        productConversationStatusCode: Int = 200,
        productConversationBody: Data = Data("{}".utf8),
        chatStatusCode: Int = 200,
        chatBody: Data = Data("{\"queued\":false}".utf8),
        configurationIdentity: APIConfigurationIdentity? = nil
    ) -> (api: PhoenixAPI, registration: UUID) {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TestURLProtocol.self]
        let registration = TestURLProtocol.install(host: host) { (request: URLRequest) in
            probe.record(request)
            let url = request.url!
            if request.httpMethod == "GET", url.path.hasPrefix("/api/product-conversations") {
                let response = HTTPURLResponse(url: url, statusCode: productConversationStatusCode, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
                return (response, productConversationBody)
            }
            if request.httpMethod == "POST", url.path.contains("/chat") {
                let response = HTTPURLResponse(url: url, statusCode: chatStatusCode, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
                return (response, chatBody)
            }
            let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
            return (response, Data("{}".utf8))
        }
        let session = URLSession(configuration: configuration)
        let api = PhoenixAPI(
            baseURL: URL(string: "https://\(host)")!,
            password: nil,
            allowSelfSigned: false,
            configurationIdentity: configurationIdentity ?? APIConfigurationIdentity(serverURL: "https://\(host)", credentialGeneration: host, trustSelfSigned: false),
            session: session,
            streamSession: session)!
        return (api, registration)
    }

    func testBackgroundAttentionPreservesCoordinatorProjectionAliasesAndCache() async throws {
        let baseDirectory = isolatedDiskDirectory()
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory)
        let probe = SendProbe()
        let listBody = try JSONEncoder().encode(ProductConversationListResponse(
            product_conversations: [
                .init(
                    product_conversation_id: "pc-ordinary",
                    canonical_route: "/product-conversations/pc-ordinary",
                    canonical_root: .init(
                        transcript_row_id: "ordinary-row", slug: "ordinary", title: "Ordinary"),
                    ordinary_lifecycle: .open,
                    latest_transcript_row_id: "ordinary-row",
                    updated_at: "2025-01-02T04:04:05Z",
                    presentation: .state(displayName: "Ordinary", presentationMode: "idle"))
            ]))
        let (api, registration) = makeHTTPAPI(
            probe: probe, productConversationBody: listBody)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let coordinatorStore = InMemoryCoordinatorIdentityStore(
            "coordinator-row", configurationIdentity: api.configurationIdentity)
        let model = AppModel(
            conversationPersistenceStore: store,
            coordinatorIdentityStore: coordinatorStore,
            credentialStore: InMemoryCredentialStore())
        model.replaceAPIForTesting(api)
        let coordinator = conversation(id: "coordinator-row")
        model.listStore.upsert(coordinator)
        model.enableBackgroundNudgesForTesting()

        let succeeded = await model.runBackgroundAttentionCheck()

        XCTAssertTrue(succeeded, model.listStore.lastError ?? "background attention failed")
        XCTAssertTrue(model.listStore.conversations.contains { $0.id == "coordinator-row" })
        XCTAssertEqual(
            model.listStore.aggregateId(forTranscriptRowId: "coordinator-row"),
            coordinator.aggregateIdentity)
        let restored = ConversationListStore(
            hasCachedSnapshot: { _ in false }, context: store.listPersistenceContext!)
        XCTAssertTrue(restored.conversations.contains { $0.id == "coordinator-row" })
        XCTAssertEqual(
            restored.aggregateId(forTranscriptRowId: "coordinator-row"),
            coordinator.aggregateIdentity)
    }

    func testBackgroundAttentionLatestSuccessorInvalidatesStoppedSingleSegmentCloseCardinality() async throws {
        let probe = SendProbe()
        let listBody = try JSONEncoder().encode(ProductConversationListResponse(
            product_conversations: [
                .init(
                    product_conversation_id: "pc-1",
                    canonical_route: "/product-conversations/pc-1",
                    canonical_root: .init(
                        transcript_row_id: "row-1", slug: "root", title: "Root"),
                    ordinary_lifecycle: .open,
                    latest_transcript_row_id: "row-2",
                    updated_at: "2025-01-02T04:04:05Z",
                    presentation: .state(displayName: "Root", presentationMode: "working"))
            ]))
        let (api, registration) = makeHTTPAPI(
            probe: probe, productConversationBody: listBody)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel()
        model.replaceAPIForTesting(api)
        let detail = model.productConversationDetailModel(for: "pc-1")
        detail.applyForTesting(testSingleSegmentProductConversationSnapshot())
        detail.stop()
        model.enableBackgroundNudgesForTesting()

        let succeeded = await model.runBackgroundAttentionCheck()

        XCTAssertTrue(succeeded, model.listStore.lastError ?? "background attention failed")
        let root = try XCTUnwrap(model.listStore.conversations.first {
            $0.aggregateIdentity == "pc-1"
        })
        XCTAssertEqual(root.id, "row-1")
        XCTAssertEqual(
            model.closeUnavailableExplanation(for: root),
            "Open the conversation before closing it.")
        let archived = await model.archive(conversationId: "row-1")
        XCTAssertFalse(archived)
        XCTAssertTrue(probe.archivePostPaths.isEmpty)
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

    func testInstallAPIForTestingInvalidatesInheritedAPIWorkBeforeReplacement() {
        let originalURL = UserDefaults.standard.string(forKey: "phoenix.serverURL")
        UserDefaults.standard.set("http://127.0.0.1:2", forKey: "phoenix.serverURL")
        defer {
            if let originalURL {
                UserDefaults.standard.set(originalURL, forKey: "phoenix.serverURL")
            } else {
                UserDefaults.standard.removeObject(forKey: "phoenix.serverURL")
            }
        }
        let model = AppModel()
        let inheritedGeneration = model.apiGenerationForTesting
        XCTAssertTrue(model.aggregateEventStreamOwnedForTesting)
        XCTAssertNotNil(model.aggregateReconciliationId)

        model.installAPIForTesting()

        XCTAssertGreaterThan(model.apiGenerationForTesting, inheritedGeneration)
        XCTAssertFalse(model.aggregateEventStreamOwnedForTesting)
        XCTAssertNil(model.aggregateReconciliationId)
    }

    func testConnectivityRestoreDefersReconciliationUntilStreamIsOpen() {
        let model = AppModel()
        model.installAPIForTesting()
        model.connectivity.setOnlineForTesting(false)
        model.startAggregateEventStreamForTesting()
        XCTAssertTrue(model.aggregateEventStreamOwnedForTesting)

        model.connectivity.setOnlineForTesting(true)

        XCTAssertTrue(model.aggregateEventStreamOwnedForTesting)
        XCTAssertNil(model.aggregateReconciliationId)
    }

    func testProductConversationDeleteOutcomeCarriesEveryAuthoritativeMember() throws {
        let response = try JSONDecoder().decode(
            PhoenixAPI.ProductConversationDeleteResponse.self,
            from: Data("""
            {"success":true,"outcome":{"type":"deleted","deleted_conversation_ids":["root","agent"]}}
            """.utf8))

        XCTAssertEqual(
            response.outcome,
            .deleted(conversationIds: ["root", "agent"]))
    }

    func testForegroundRestoreDefersReconciliationUntilStreamIsReady() {
        let model = AppModel()
        model.installAPIForTesting()
        model.backgrounded()

        model.foregrounded()

        XCTAssertTrue(model.aggregateEventStreamOwnedForTesting)
        XCTAssertNil(model.aggregateReconciliationId)
    }

    func testStaleAggregateReconciliationCannotOverwriteNewerAppliedList() async {
        let model = AppModel()
        model.installAPIForTesting()
        let staleId = model.prepareAggregateReconciliationForTesting()
        let currentId = model.prepareAggregateReconciliationForTesting()
        let current = [conversation(id: "new", aggregateId: "pc-new")]
        let stale = [conversation(id: "old", aggregateId: "pc-old")]

        let currentApplied = await model.applyAggregateListForReconciliationForTesting(
            current,
            reconciliationId: currentId)
        let staleApplied = await model.applyAggregateListForReconciliationForTesting(
            stale,
            reconciliationId: staleId)
        XCTAssertTrue(currentApplied)
        XCTAssertFalse(staleApplied)
        XCTAssertEqual(model.listStore.conversations, current)
    }

    func testCancelledAggregateReconciliationCannotApplyFetchedList() async {
        let model = AppModel()
        model.installAPIForTesting()
        let reconciliationId = model.prepareAggregateReconciliationForTesting()
        let stale = [conversation(id: "old", aggregateId: "pc-old")]
        let task = Task { @MainActor in
            while !Task.isCancelled { await Task.yield() }
            return await model.applyAggregateListForReconciliationForTesting(
                stale,
                reconciliationId: reconciliationId)
        }

        task.cancel()

        let applied = await task.value
        XCTAssertFalse(applied)
        XCTAssertTrue(model.listStore.conversations.isEmpty)
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

    func testAggregateReconciliationPreservesProvisioningShellOmittedFromProductList() async {
        let model = AppModel()
        model.installAPIForTesting()
        let shell = conversation(
            id: "shell-row",
            aggregateId: "pc-shell",
            state: .object([
                "type": .string("provisioning"),
                "job_id": .string("creation-job"),
            ]))
        model.listStore.upsert(shell)
        let reconciliationId = model.prepareAggregateReconciliationForTesting()

        let applied = await model.applyAggregateListForReconciliationForTesting(
            [],
            reconciliationId: reconciliationId)
        XCTAssertTrue(applied)
        XCTAssertEqual(model.listStore.conversations, [shell])
        XCTAssertEqual(
            AppModel.removedAggregateIds(
                authoritative: [],
                locallyOwned: ["pc-shell", "pc-deleted"],
                preserving: ["pc-shell"]),
            ["pc-deleted"])
    }

    func testLocallyOwnedAggregatesExcludeRememberedLegacyCoordinatorWithoutRuntimeRole() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-legacy-coordinator-tests-\(UUID().uuidString)")
        UserDefaults.standard.set("coordinator-row", forKey: "phoenix.coordinatorConversationId")
        defer { UserDefaults.standard.removeObject(forKey: "phoenix.coordinatorConversationId") }
        persistReadableSnapshot(conversation: conversation(
            id: "coordinator-row",
            aggregateId: "pc-coordinator"))
        let model = AppModel()
        model.listStore.upsert(conversation(
            id: "coordinator-row",
            aggregateId: "pc-coordinator"))
        model.listStore.upsert(conversation(id: "ordinary-row", aggregateId: "pc-ordinary"))

        let owned = model.locallyOwnedOrdinaryAggregatesForTesting()

        XCTAssertNil(owned["pc-coordinator"])
        XCTAssertEqual(owned["pc-ordinary"], ["ordinary-row"])
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

    func testAggregateDeletionTerminalizesRetainedOwnersMissingFromAliases() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-retained-delete-tests-\(UUID().uuidString)")
        let aggregateId = "pc-deleted"
        let openRow = conversation(id: "open-row", aggregateId: aggregateId)
        let drainRow = conversation(id: "drain-row", aggregateId: aggregateId)
        persistReadableSnapshot(conversation: openRow)
        persistReadableSnapshot(conversation: drainRow)
        let model = AppModel()
        model.installAPIForTesting()
        let openOwner = try XCTUnwrap(model.session(for: openRow.id))
        let drainOwner = try XCTUnwrap(model.installDrainSessionForTesting(conversationId: drainRow.id))

        let removed = await model.removeProductHistoryLocallyForTesting(
            productConversationId: aggregateId,
            transcriptIds: [])

        XCTAssertTrue(removed)
        XCTAssertTrue(openOwner.isHardDeleted)
        XCTAssertTrue(drainOwner.isHardDeleted)
        XCTAssertFalse(openOwner.acceptsConversationActions)
        XCTAssertFalse(drainOwner.acceptsConversationActions)
        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: openRow.id))
        XCTAssertFalse(ConversationSession.hasCachedSnapshot(conversationId: drainRow.id))
    }

    func testAggregateDeletionTombstonesEveryIdentityBeforeCleanupCanSuspend() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-delete-tombstone-race-tests-\(UUID().uuidString)")
        let aggregateId = "pc-deleted"
        let listed = conversation(id: "listed-row", aggregateId: aggregateId)
        let cached = conversation(id: "cached-row", aggregateId: aggregateId)
        let retained = conversation(id: "retained-row", aggregateId: aggregateId)
        persistReadableSnapshot(conversation: cached)
        persistReadableSnapshot(conversation: retained)
        let model = AppModel()
        model.installAPIForTesting()
        model.listStore.upsert(listed)
        XCTAssertNotNil(model.session(for: retained.id))
        var checkedBeforeFirstAwait = false

        let removed = await model.removeProductHistoryLocallyForTesting(
            productConversationId: aggregateId,
            transcriptIds: [listed.id, cached.id],
            tombstonesInstalled: {
                checkedBeforeFirstAwait = true
                XCTAssertTrue(model.deletedProductHistoryIds.isSuperset(
                    of: [aggregateId, listed.id, cached.id, retained.id]))
                XCTAssertNil(model.session(for: aggregateId))
                XCTAssertNil(model.session(for: listed.id))
                XCTAssertNil(model.session(for: cached.id))
                XCTAssertNil(model.session(for: retained.id))
            })

        XCTAssertTrue(removed)
        XCTAssertTrue(checkedBeforeFirstAwait)
        XCTAssertNil(model.session(for: listed.id))
        XCTAssertNil(model.session(for: cached.id))
        XCTAssertNil(model.session(for: retained.id))
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

    func testCloseRehydrationFencesEveryListedActiveAggregateBeforeSelectingPrompt() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-close-rehydration-fence-tests-\(UUID().uuidString)")
        let model = AppModel()
        model.installAPIForTesting()
        let first = conversation(
            id: "row-a",
            aggregateId: "product-a",
            closeAction: .unavailable(reason: .active_close_attempt))
        let second = conversation(
            id: "row-b",
            aggregateId: "product-b",
            closeAction: .unavailable(reason: .active_close_attempt))
        model.listStore.upsert(first)
        model.listStore.upsert(second)
        let firstSession = try XCTUnwrap(model.session(for: first.id))
        let secondSession = try XCTUnwrap(model.session(for: second.id))
        var fetched: [String] = []

        await model.rehydratePendingProductCloseConfirmationForTesting { aggregateId in
            XCTAssertTrue(firstSession.isArchiving)
            XCTAssertTrue(secondSession.isArchiving)
            fetched.append(aggregateId)
            var snapshot = self.historySnapshot(aggregateId: aggregateId, segments: [])
            snapshot.ordinary_lifecycle = .open
            snapshot.close = self.closeSnapshot(phase: .awaiting_stop_work_confirmation)
            return snapshot
        }

        XCTAssertEqual(fetched, ["product-a", "product-b"])
        XCTAssertEqual(model.pendingProductCloseConfirmation?.productConversationId, "product-a")
        XCTAssertFalse(firstSession.acceptsConversationActions)
        XCTAssertFalse(secondSession.acceptsConversationActions)
    }

    func testCloseRehydrationClearsAbsentFencePromptAndReconciliationButPreservesActive() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-authoritative-close-tests-\(UUID().uuidString)")
        let model = AppModel()
        model.installAPIForTesting()
        let absent = conversation(id: "absent-row", aggregateId: "absent")
        let active = conversation(
            id: "active-row",
            aggregateId: "active",
            closeAction: .unavailable(reason: .active_close_attempt))
        model.listStore.upsert(absent)
        model.listStore.upsert(active)
        let absentSession = try XCTUnwrap(model.session(for: absent.id))
        let activeSession = try XCTUnwrap(model.session(for: active.id))
        model.recordCloseConfirmationRequiredForTesting(productConversationId: "absent")
        model.installPendingProductCloseConfirmationForTesting(PendingProductCloseConfirmation(
            productConversationId: "absent",
            transcriptRowId: absent.id,
            close: closeSnapshot(phase: .awaiting_stop_work_confirmation)))
        model.fenceProductCloseForTesting(productConversationId: "active", fenced: true)

        model.listStore.remove(aggregateId: "absent")
        await model.rehydratePendingProductCloseConfirmationForTesting { aggregateId in
            XCTAssertEqual(aggregateId, "active")
            var snapshot = self.historySnapshot(aggregateId: aggregateId, segments: [])
            snapshot.ordinary_lifecycle = .open
            snapshot.close = self.closeSnapshot(phase: .settling_active_work)
            return snapshot
        }

        XCTAssertFalse(absentSession.isArchiving)
        XCTAssertTrue(absentSession.acceptsConversationActions)
        XCTAssertTrue(activeSession.isArchiving)
        XCTAssertFalse(activeSession.acceptsConversationActions)
        XCTAssertNil(model.pendingProductCloseConfirmation)
        XCTAssertFalse(model.closeConfirmationReconciliationIdsForTesting.contains("absent"))
    }

    func testAggregateEventBackoffKeepsJitterRangeBelowThirtySecondCap() {
        var minimumBackoff = AggregateEventStreamBackoff()
        var maximumBackoff = AggregateEventStreamBackoff()

        let minimums = (0..<7).map { _ in
            minimumBackoff.delayAfterDisconnect(streamWasHealthy: false, jitterFraction: 0)
        }
        let maximums = (0..<7).map { _ in
            maximumBackoff.delayAfterDisconnect(streamWasHealthy: false, jitterFraction: 1)
        }

        XCTAssertEqual(minimums, [1, 2, 4, 8, 16, 21, 21])
        XCTAssertEqual(maximums, [1.3, 2.6, 5.2, 10.4, 20.8, 30, 30])
        XCTAssertLessThan(minimums.last!, maximums.last!)
        XCTAssertLessThanOrEqual(maximums.last!, AggregateEventStreamBackoff.maximumDelay)
        XCTAssertEqual(
            minimumBackoff.delayAfterDisconnect(streamWasHealthy: true, jitterFraction: 0),
            1)
        XCTAssertEqual(minimumBackoff.baseDelay, 2)
    }

    func testForegroundAttentionSeedInvalidatesBackgroundEvidenceGeneration() {
        let model = AppModel()
        let backgroundGeneration = model.attentionEvidenceGenerationForTesting
        model.listStore.upsert(conversation(
            id: "visible",
            aggregateId: "pc-visible",
            mode: "needs_action"))

        model.seedForegroundAttentionForTesting()

        XCTAssertGreaterThan(model.attentionEvidenceGenerationForTesting, backgroundGeneration)
        XCTAssertEqual(
            model.attention.snapshot["pc-visible"],
            AttentionMonitor.Entry(mode: "needs_action", title: "visible"))
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
        model.installAPIForTesting()
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
        model.installAPIForTesting()
        let row = conversation(id: "latest", aggregateId: "product")
        model.listStore.upsert(row)
        model.fenceProductCloseForTesting(productConversationId: "product", fenced: true)

        let session = try XCTUnwrap(model.session(for: row.id))

        XCTAssertTrue(session.isArchiving)
        XCTAssertFalse(session.acceptsConversationActions)
    }

    func testActiveCloseFenceAppliesToCachedOwnersMissingFromListAliases() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-cached-close-fence-tests-\(UUID().uuidString)")
        persistReadableSnapshot(conversation: conversation(id: "open", aggregateId: "product"))
        persistReadableSnapshot(conversation: conversation(id: "drain", aggregateId: "product"))
        let model = AppModel()
        model.installAPIForTesting()
        let openOwner = try XCTUnwrap(model.session(for: "open"))
        let drainOwner = try XCTUnwrap(model.installDrainSessionForTesting(conversationId: "drain"))
        XCTAssertTrue(model.listStore.conversations.isEmpty)

        model.fenceProductCloseForTesting(productConversationId: "product", fenced: true)

        XCTAssertTrue(openOwner.isArchiving)
        XCTAssertTrue(drainOwner.isArchiving)
        XCTAssertFalse(openOwner.acceptsConversationActions)
        XCTAssertFalse(drainOwner.acceptsConversationActions)
    }

    func testCancelReconciliationKeepsFenceForConcurrentCloseAttempt() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-cancel-close-fence-tests-\(UUID().uuidString)")
        let row = conversation(id: "latest", aggregateId: "product")
        persistReadableSnapshot(conversation: row)
        let model = AppModel()
        model.installAPIForTesting()
        let session = try XCTUnwrap(model.session(for: row.id))
        model.fenceProductCloseForTesting(productConversationId: "product", fenced: true)
        var concurrent = historySnapshot(aggregateId: "product", segments: [])
        concurrent.ordinary_lifecycle = .open
        concurrent.close = closeSnapshot(phase: .awaiting_stop_work_confirmation)

        model.reconcileAuthoritativeCloseForTesting(
            concurrent,
            productConversationId: "product")

        XCTAssertEqual(model.pendingProductCloseConfirmation?.close.attempt_id, "attempt")
        XCTAssertTrue(session.isArchiving)
        XCTAssertFalse(session.acceptsConversationActions)
    }

    func testTypedCloseConflictFencesImmediatelyAndRetainsReconciliationObligation() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-close-conflict-fence-tests-\(UUID().uuidString)")
        let row = conversation(id: "latest", aggregateId: "product")
        persistReadableSnapshot(conversation: row)
        let model = AppModel()
        model.installAPIForTesting()
        let session = try XCTUnwrap(model.session(for: row.id))

        model.recordCloseConfirmationRequiredForTesting(productConversationId: "product")

        XCTAssertTrue(session.isArchiving)
        XCTAssertFalse(session.acceptsConversationActions)
        XCTAssertEqual(model.closeConfirmationReconciliationIdsForTesting, ["product"])
        XCTAssertNil(model.pendingProductCloseConfirmation)
    }

    func testTypedCloseConflictReconciliationUnfencesOnlyAfterAuthoritativeSnapshot() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-close-conflict-reconcile-tests-\(UUID().uuidString)")
        let row = conversation(id: "latest", aggregateId: "product")
        persistReadableSnapshot(conversation: row)
        let model = AppModel()
        model.installAPIForTesting()
        let session = try XCTUnwrap(model.session(for: row.id))
        model.recordCloseConfirmationRequiredForTesting(productConversationId: "product")
        var open = historySnapshot(aggregateId: "product", segments: [])
        open.ordinary_lifecycle = .open

        model.completeCloseConfirmationReconciliationForTesting(
            open,
            productConversationId: "product")

        XCTAssertFalse(session.isArchiving)
        XCTAssertTrue(session.acceptsConversationActions)
        XCTAssertTrue(model.closeConfirmationReconciliationIdsForTesting.isEmpty)
        XCTAssertNil(model.pendingProductCloseConfirmation)
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

    func testLegacyCachedHistoryResolvesCanonicalRootBeforeDelete() async throws {
        let legacy = conversation(id: "latest", aggregateId: "product", archived: true)
        var fetchedReferences: [String] = []

        let root = try await AppModel.resolveProductHistoryRoot(
            conversation: legacy,
            fetch: { reference in
                fetchedReferences.append(reference)
                return self.historySnapshot(aggregateId: "product", segments: [])
            })

        XCTAssertEqual(root, "root")
        XCTAssertEqual(fetchedReferences, ["latest"])
    }

    func testPersistedHistoryRootSkipsCanonicalLookupBeforeDelete() async throws {
        let migrated = conversation(
            id: "latest",
            aggregateId: "product",
            archived: true,
            chainRootId: "persisted-root")

        let root = try await AppModel.resolveProductHistoryRoot(
            conversation: migrated,
            fetch: { _ in
                XCTFail("A persisted canonical root must not be looked up again")
                throw APIError.invalidURL
            })

        XCTAssertEqual(root, "persisted-root")
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
        let model = makeModel()
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
        let model = makeModel()
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
        let model = makeModel()
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
        let bootstrap = makeModel(
            hasCachedSnapshot: { $0 == "coordinator-row" })
        let identity = bootstrap.configurationIdentity!
        let identityStore = InMemoryCoordinatorIdentityStore("coordinator-row", configurationIdentity: identity)
        let model = makeModel(
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
        let model = makeModel(hasCachedSnapshot: { id in id == "row-1" })
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
        let model = makeModel(hasCachedSnapshot: { id in id == "row-1" })
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
        let model = makeModel(hasCachedSnapshot: { id in id == "row-1" })
        model.listStore.upsert(predecessor)
        model.listStore.upsert(self.conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        let aggregateConversation = model.listStore.conversations.first!
        model.connectivity.setOnlineForTesting(false)

        XCTAssertEqual(model.navigationConversationId(for: aggregateConversation), "row-1")
    }

    func testAggregateNavigationDestinationRemainsAggregateAcrossConnectivityChanges() {
        let model = makeModel(hasCachedSnapshot: { $0 == "row-1" })
        let aggregate = conversation(id: "row-2", aggregateId: "pc-1")
        model.listStore.upsert(conversation(id: "row-1", aggregateId: "pc-1"))
        model.listStore.upsert(aggregate)
        model.connectivity.setOnlineForTesting(true)
        let selected = model.navigationDestination(for: aggregate)

        model.connectivity.setOnlineForTesting(false)

        guard case .aggregate(let aggregateId, let initialTranscriptRowId) = selected else {
            return XCTFail("expected aggregate destination")
        }
        XCTAssertEqual(aggregateId, "pc-1")
        XCTAssertEqual(initialTranscriptRowId, "row-1")
        guard case .aggregate = model.navigationDestination(for: aggregate) else {
            return XCTFail("offline selection must remain on aggregate detail")
        }
    }

    func testOfflineNavigationUsesCachedAggregateMemberAfterRestart() {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)", isDirectory: true)
        let predecessor = self.conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        self.persistReadableSnapshot(conversation: predecessor, baseDirectory: baseDirectory)
        let first = makeModel(hasCachedSnapshot: { id in id == "row-1" })
        first.listStore.upsert(predecessor)
        first.listStore.applyExternal(
            [self.conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root")],
            startedAt: first.listStore.externalRefreshToken())

        let reloaded = makeModel(hasCachedSnapshot: { id in id == "row-1" })
        reloaded.connectivity.setOnlineForTesting(false)
        let aggregateConversation = reloaded.listStore.conversations.first!

        XCTAssertEqual(reloaded.navigationConversationId(for: aggregateConversation), "row-1")
    }
    func testColdRestartUsesPersistedAggregateAuthorityWithoutSnapshotOrListAlias() async {
        let baseDirectory = isolatedDiskDirectory()
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory)
        let pending = makePendingOutboxEntry(conversationId: "row-2")
        let handle = store.outboxPersistence(
            conversationId: "row-2",
            aggregateAuthority: "pc-1",
            scope: defaultPersistenceScope)
        _ = await handle.save(
            PersistedOutboxEnvelope(
                scope: defaultPersistenceScope,
                aggregateAuthority: "pc-1",
                entries: [pending]),
            revision: handle.reserveRevision())
        let model = makeModel(conversationPersistenceStore: store)
        let (api, registration) = makeHTTPAPI(
            probe: SendProbe(),
            host: "example.com",
            configurationIdentity: .init(
                serverURL: "https://example.com",
                credentialGeneration: "test-default",
                trustSelfSigned: false))
        defer { TestURLProtocol.uninstall(host: "example.com", owner: registration) }
        model.replaceAPIForTesting(api)
        model.triggerStartupHardDeleteRecoveryForTesting()
        await model.awaitStartupHardDeleteRecoveryForTesting()
        model.triggerPersistedOutboxDrainIfNeededForTesting()

        _ = await model.awaitCurrentPersistedOutboxDrainForTesting()
        let session = try! XCTUnwrap(model.existingSession(for: "row-2"))

        XCTAssertEqual(session.outbox.aggregateAuthority, "pc-1")
        XCTAssertEqual(session.outbox.entries.map(\.id), [pending.id])
        XCTAssertFalse(session.canSendPersistedOutbox)
        session.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-2", aggregateId: "pc-1"),
            messages: [], agentWorking: false, presentationMode: "idle",
            lastSequenceId: 0, pendingAnchorSequenceId: 0,
            pendingEvents: [], pendingTruncated: false)))
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        XCTAssertTrue(session.canSendPersistedOutbox)
    }

    func testColdLaunchLeavesLegacyOutboxUndrainedWithoutIdentitySnapshot() async {
        _ = isolatedDiskDirectory()
        let store = TestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])])
        let model = makeModel(conversationPersistenceStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)

        let result = await model.awaitCurrentPersistedOutboxDrainForTesting()
        let session = model.existingSession(for: "row-1")

        if case .completed = result {
        } else {
            XCTFail("expected scheduled drain to complete without sending")
        }
        XCTAssertNotNil(session)
        XCTAssertNil(session?.authoritativeSnapshotReceipt)
        XCTAssertFalse(session?.canSendPersistedOutbox ?? true)
        if case .accessible(_, _, let entries) = store.inspectOutbox(conversationId: "row-1").state {
            XCTAssertEqual(entries.count, 1)
        } else {
            XCTFail("expected outbox entry to remain durable")
        }
        XCTAssertTrue(store.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
    }

    @MainActor
    func testTrustOnlyRebuildKeepsPersistedOutboxDrainAuthorityWithoutAcceptingStaleLiveIdentity() async {
        let baseDirectory = isolatedDiskDirectory()
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        let beforeTrustToggle = APIConfigurationIdentity(
            serverURL: "https://trust-toggle.invalid",
            credentialGeneration: "same-credential",
            trustSelfSigned: false)
        let afterTrustToggle = APIConfigurationIdentity(
            serverURL: beforeTrustToggle.serverURL,
            credentialGeneration: beforeTrustToggle.credentialGeneration,
            trustSelfSigned: true)
        let outbox = store.outboxPersistence(
            conversationId: "row-1",
            aggregateAuthority: "pc-1",
            scope: beforeTrustToggle.persistenceScope)
        _ = await outbox.save(
            .init(
                scope: beforeTrustToggle.persistenceScope,
                aggregateAuthority: "pc-1",
                entries: [makePendingOutboxEntry(conversationId: "row-1")]),
            revision: outbox.reserveRevision())
        let snapshot = ConversationSession.PersistedSnapshot(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [],
            lastSequenceId: 0,
            transcriptGeneration: 1,
            syncedAt: Date(),
            authoritative: .init(
                configurationIdentity: beforeTrustToggle,
                aggregateAuthority: "pc-1",
                syncedAt: Date()))
        let snapshotWriter = store.snapshotPersistence(conversationId: "row-1")
        _ = await snapshotWriter.save(snapshot, revision: snapshotWriter.reserveRevision())
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(
            probe: probe,
            host: "trust-toggle.invalid",
            configurationIdentity: afterTrustToggle)
        defer { TestURLProtocol.uninstall(host: "trust-toggle.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.connectivity.setOnlineForTesting(true)
        model.replaceAPIForTesting(api)
        _ = await model.awaitCurrentPersistedOutboxDrainForTesting()

        XCTAssertEqual(probe.chatPostPaths, ["/api/conversations/row-1/chat"])
    }

    func testAuthoritativeReceiptUnlocksOneSendAndAwaitsReflection() async {
        _ = isolatedDiskDirectory()
        let seededEntry = makePendingOutboxEntry(conversationId: "row-1")
        let probe = SendProbe()
        let host = "appmodel-send-1.invalid"
        let (api, registration) = makeHTTPAPI(probe: probe, host: host)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let store = TestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([seededEntry])],
            aggregateMembersById: ["pc-1": ["row-1"]])
        let model = makeModel(conversationPersistenceStore: store)
        model.listStore.upsert(self.conversation(id: "row-1", aggregateId: "pc-1", title: "Aggregate member"))
        model.replaceAPIForTesting(api)
        let session = model.existingSession(for: "row-1") ?? model.session(for: "row-1")
        session?.receive(.initSnapshot(.init(
            conversation: self.conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [],
            agentWorking: false,
            presentationMode: "idle",
            lastSequenceId: 0,
            pendingAnchorSequenceId: 0,
            pendingEvents: [],
            pendingTruncated: false)))
        _ = await session?.flushSnapshotPersistence()
        guard let generation = session?.drainOutbox() else {
            XCTFail("expected receipt-unlocked outbox drain generation")
            return
        }
        let drained = await session!.awaitDrainOutbox(generation: generation)
        XCTAssertTrue(drained)
        _ = await session!.outbox.flushPersistence()

        XCTAssertEqual(probe.chatPostPaths.count, 1)
        if case .accessible(_, _, let entries) = store.inspectOutbox(conversationId: "row-1").state {
            XCTAssertEqual(entries.count, 1)
            XCTAssertEqual(entries[0].localId, seededEntry.localId)
            XCTAssertTrue(entries[0].acceptedByServer)
            XCTAssertEqual(entries[0].status, .pending)
            XCTAssertEqual(entries[0].attemptCount, seededEntry.attemptCount + 1)
            XCTAssertNil(entries[0].lastError)
        } else {
            XCTFail("expected durable outbox contents after send inspection")
        }
    }

    func testAuthoritativeReflectionReconcilesDurablyAfterReceiptUnlockedSend() async {
        _ = isolatedDiskDirectory()
        let seededEntry = makePendingOutboxEntry(conversationId: "row-1")
        let probe = SendProbe()
        let host = "appmodel-send-2.invalid"
        let (api, registration) = makeHTTPAPI(probe: probe, host: host)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let store = TestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([seededEntry])],
            aggregateMembersById: ["pc-1": ["row-1"]])
        let model = makeModel(conversationPersistenceStore: store)
        model.listStore.upsert(self.conversation(id: "row-1", aggregateId: "pc-1", title: "Aggregate member"))
        model.replaceAPIForTesting(api)
        let session = model.existingSession(for: "row-1") ?? model.session(for: "row-1")
        session?.receive(.initSnapshot(.init(
            conversation: self.conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [],
            agentWorking: false,
            presentationMode: "idle",
            lastSequenceId: 0,
            pendingAnchorSequenceId: 0,
            pendingEvents: [],
            pendingTruncated: false)))
        _ = await session?.flushSnapshotPersistence()
        guard let generation = session?.drainOutbox() else {
            XCTFail("expected receipt-unlocked outbox drain generation")
            return
        }
        let drained = await session!.awaitDrainOutbox(generation: generation)
        XCTAssertTrue(drained)
        _ = await session!.outbox.flushPersistence()

        let reflected = Message(
            message_id: seededEntry.localId,
            conversation_id: "row-1",
            sequence_id: 1,
            message_type: "user",
            content: .string(seededEntry.text),
            display_data: nil,
            created_at: "2026-01-01T00:00:00Z")
        session?.receive(.message(seq: 1, message: reflected))
        XCTAssertEqual(probe.chatPostPaths.count, 1)
        XCTAssertTrue(session?.outbox.visibleEntries.isEmpty ?? false)
        XCTAssertEqual(session?.outbox.entries.first?.status, .pending)
        let snapshotPersisted = await session?.flushSnapshotPersistence()
        XCTAssertEqual(snapshotPersisted, true)
        XCTAssertEqual(session?.outbox.entries.first?.status, .reconciled)
        _ = await session?.outbox.flushPersistence()
        guard let repeatedGeneration = session?.drainOutbox() else {
            XCTFail("expected repeat drain generation for idempotency check")
            return
        }
        let drainedAgain = await session!.awaitDrainOutbox(generation: repeatedGeneration)
        XCTAssertTrue(drainedAgain)

        XCTAssertEqual(probe.chatPostPaths.count, 1)
        switch store.inspectOutbox(conversationId: "row-1").state {
        case .missing:
            break
        case .accessible(_, _, let entries):
            XCTAssertTrue(entries.allSatisfy { !$0.isVisible })
            XCTAssertTrue(entries.allSatisfy { $0.status == .reconciled })
        case .inaccessible, .incompatibleNewerVersion:
            XCTFail("expected reflected outbox state to stay readable")
        }
    }

    func testColdLaunchAlreadyOnlineDrainsPersistedOutboxWithIdentitySnapshot() async {
        _ = isolatedDiskDirectory()
        let probe = SendProbe()
        let host = "appmodel-send-3.invalid"
        let (api, registration) = makeHTTPAPI(probe: probe, host: host)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let gate = AsyncCandidateGate()
        let store = GatedConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            aggregateMembersById: ["pc-1": ["row-1"]],
            gate: gate)
        let model = makeModel(conversationPersistenceStore: store)
        model.listStore.upsert(self.conversation(id: "row-1", aggregateId: "pc-1", title: "Aggregate member"))
        model.connectivity.setOnlineForTesting(false)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        model.replaceAPIForTesting(api)

        model.triggerStartupHardDeleteRecoveryForTesting()
        await model.awaitStartupHardDeleteRecoveryForTesting()
        model.connectivity.setOnlineForTesting(true)
        await gate.waitForEntry()
        guard let generation = model.currentPersistedOutboxDrainGenerationForTesting() else {
            XCTFail("expected startup drain generation after gated discovery entry")
            return
        }
        let session = model.existingSession(for: "row-1") ?? model.session(for: "row-1")
        session?.receive(.initSnapshot(.init(
            conversation: self.conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [],
            agentWorking: false,
            presentationMode: "idle",
            lastSequenceId: 0,
            pendingAnchorSequenceId: 0,
            pendingEvents: [],
            pendingTruncated: false)))
        _ = await session?.flushSnapshotPersistence()
        await gate.release()

        let result = await model.awaitPersistedOutboxDrainForTesting(generation: generation)
        XCTAssertEqual(result, .completed(generation))
        let owningSession = model.existingSession(for: "row-1")
        XCTAssertTrue(session === owningSession)
        XCTAssertEqual(probe.chatPostPaths.count, 1)
        if case .accessible(_, _, let entries) = store.inspectOutbox(conversationId: "row-1").state {
            XCTAssertEqual(entries.count, 1)
            XCTAssertTrue(entries[0].acceptedByServer)
            XCTAssertEqual(entries[0].status, .pending)
        } else {
            XCTFail("expected durable outbox contents after startup drain")
        }
    }

    func testConnectivityRestoreReclassifiesTransientInaccessibleHardDeleteFenceBeforeOneDrain() async {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: [
                "row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])
            ],
            snapshotsByConversationId: ["row-1"],
            aggregateMembersById: ["pc-1": ["row-1"]])
        store.hardDeleteFenceLoadResult = .inaccessible
        let gate = AsyncCandidateGate()
        store.onPendingOutboxOwnerTranscriptRowIds = {
            await gate.markEntered()
            await gate.awaitRelease()
            return ["row-1"]
        }
        let probe = SendProbe()
        let host = "transient-fence-recovery.invalid"
        let (api, registration) = makeHTTPAPI(probe: probe, host: host)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let snapshotWriter = store.snapshotPersistence(conversationId: "row-1")
        let snapshotSyncedAt = Date()
        _ = await snapshotWriter.save(
            ConversationSession.PersistedSnapshot(
                conversation: conversation(id: "row-1", aggregateId: "pc-1"),
                messages: [],
                lastSequenceId: 0,
                transcriptGeneration: nil,
                syncedAt: snapshotSyncedAt,
                authoritative: ConversationSession.PersistedSnapshotAuthority(
                    configurationIdentity: api.configurationIdentity,
                    aggregateAuthority: "pc-1",
                    syncedAt: snapshotSyncedAt)),
            revision: snapshotWriter.reserveRevision())
        let model = AppModel(
            conversationPersistenceStore: store,
            credentialStore: InMemoryCredentialStore())
        model.connectivity.setOnlineForTesting(false)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        model.replaceAPIForTesting(api)
        model.listStore.upsert(conversation(id: "row-1", aggregateId: "pc-1"))

        let inaccessibleLoadCount = store.hardDeleteFenceLoadCount
        XCTAssertGreaterThanOrEqual(inaccessibleLoadCount, 1)
        XCTAssertEqual(store.pendingOutboxDiscoveryCount, 0)
        XCTAssertTrue(probe.chatPostPaths.isEmpty)

        let startupDrainGeneration = model.currentPersistedOutboxDrainGenerationForTesting()
        if let startupDrainGeneration {
            _ = await model.awaitPersistedOutboxDrainForTesting(generation: startupDrainGeneration)
        }
        let session = model.session(for: "row-1")
        XCTAssertNotNil(session)

        store.hardDeleteFenceLoadResult = .accessible([])
        model.connectivity.setOnlineForTesting(true)
        await gate.waitForEntry()
        XCTAssertTrue(probe.chatPostPaths.isEmpty)
        guard let generation = model.currentPersistedOutboxDrainGenerationForTesting() else {
            XCTFail("expected connectivity recovery to schedule a persisted outbox drain")
            return
        }
        await gate.release()
        let drain = await model.awaitPersistedOutboxDrainForTesting(generation: generation)

        XCTAssertEqual(drain, .completed(generation))
        let sessionDrainGeneration = session?.currentDrainGenerationForTesting()
        if let sessionDrainGeneration {
            _ = await session?.awaitDrainOutbox(generation: sessionDrainGeneration)
        }
        XCTAssertEqual(store.hardDeleteFenceLoadCount, inaccessibleLoadCount + 1)
        XCTAssertEqual(store.pendingOutboxDiscoveryCount, 1)
        XCTAssertEqual(probe.chatPostPaths, ["/api/conversations/row-1/chat"])
    }

    func testOfflineLaunchWaitsAndThenDrainsOnceWhenConnectivityRestores() async {
        let gate = AsyncCandidateGate()
        let store = GatedConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            gate: gate)
        let model = makeModel(conversationPersistenceStore: store)
        model.connectivity.setOnlineForTesting(false)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let (injectedAPI, registration) = makeHTTPAPI(probe: SendProbe())
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        model.replaceAPIForTesting(injectedAPI)
        let notReady = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(notReady, .notReady)
        XCTAssertNil(model.existingSession(for: "row-1"))

        model.triggerStartupHardDeleteRecoveryForTesting()
        await model.awaitStartupHardDeleteRecoveryForTesting()
        model.connectivity.setOnlineForTesting(true)
        await gate.waitForEntry()
        guard let generation = model.currentPersistedOutboxDrainGenerationForTesting() else {
            XCTFail("expected drain generation after connectivity restores")
            return
        }
        XCTAssertNotNil(model.existingSession(for: "row-1") ?? model.session(for: "row-1"))
        await gate.release()
        let result = await model.awaitPersistedOutboxDrainForTesting(generation: generation)
        XCTAssertEqual(result, .completed(generation))
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
        let store = TestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])])
        let model = makeModel(conversationPersistenceStore: store)
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
        let gate = AsyncCandidateGate()
        let gatedStore = GatedConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            gate: gate)
        let model = makeModel(conversationPersistenceStore: gatedStore)
        model.configureForTesting(serverURL: "", trustSelfSigned: true)
        let notReady = await model.awaitCurrentPersistedOutboxDrainForTesting()
        XCTAssertEqual(notReady, .notReady)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        await gate.waitForEntry()
        guard let generation = model.currentPersistedOutboxDrainGenerationForTesting() else {
            XCTFail("expected drain generation after api becomes ready")
            return
        }
        await gate.release()
        let result = await model.awaitPersistedOutboxDrainForTesting(generation: generation)
        XCTAssertEqual(result, .completed(generation))
        XCTAssertNotNil(model.existingSession(for: "row-1"))
    }

    func testReconfigurationCancelsObservationAndSchedulesNewDrainGeneration() async {
        let store = TestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])])
        let model = makeModel(conversationPersistenceStore: store)
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
    func testBlockedCandidateDiscoveryAcrossAPIReplacementDoesNotCreateStaleSessionOrReschedule() async {
        let gate = AsyncCandidateGate()
        let store = GatedConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            gate: gate)
        let model = makeModel(conversationPersistenceStore: store)
        model.listStore.upsert(self.conversation(id: "row-1", aggregateId: "pc-1", title: "Aggregate member"))
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let firstGeneration = model.currentPersistedOutboxDrainGenerationForTesting()!
        await gate.waitForEntry()

        let replacementProbe = SendProbe()
        let (replacement, registration) = makeHTTPAPI(probe: replacementProbe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        model.replaceAPIForTesting(replacement)
        let replacementGeneration = model.currentPersistedOutboxDrainGenerationForTesting()
        XCTAssertNotNil(replacementGeneration)
        XCTAssertGreaterThan(replacementGeneration!, firstGeneration)
        await gate.release()

        let staleResult = await model.awaitPersistedOutboxDrainForTesting(generation: firstGeneration)
        XCTAssertEqual(staleResult, .noCurrentDrain)
        XCTAssertNil(model.existingSession(for: "row-1"))
        XCTAssertEqual(replacementProbe.chatPostPaths.count, 0)
        XCTAssertEqual(model.currentPersistedOutboxDrainGenerationForTesting(), replacementGeneration)

        let nextResult = await model.awaitPersistedOutboxDrainForTesting(generation: replacementGeneration!)
        XCTAssertEqual(nextResult, .completed(replacementGeneration!))
        XCTAssertNotNil(model.existingSession(for: "row-1"))
        XCTAssertEqual(replacementProbe.chatPostPaths.count, 0)
    }

    func testBlockedCandidateDiscoveryAcrossSignOutDoesNotRecreateSessionOrState() async {
        let gate = AsyncCandidateGate()
        let store = GatedConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            snapshotsByConversationId: ["row-1"],
            gate: gate)
        let model = makeModel(conversationPersistenceStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let firstGeneration = model.currentPersistedOutboxDrainGenerationForTesting()!
        await gate.waitForEntry()

        await model.signOut()
        XCTAssertEqual(model.serverURLString, "")
        XCTAssertNil(model.currentPersistedOutboxDrainGenerationForTesting())
        XCTAssertFalse(store.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
        if case .missing = store.inspectOutbox(conversationId: "row-1").state {
        } else {
            XCTFail("expected signOut to remove persisted outbox state")
        }
        await gate.release()

        let staleResult = await model.awaitPersistedOutboxDrainForTesting(generation: firstGeneration)
        XCTAssertEqual(staleResult, .notReady)
        XCTAssertNil(model.existingSession(for: "row-1"))
        XCTAssertNil(model.api)
    }

    func testConfigurePersistsAtomicCredentialRecordAndRelaunchesWithoutPasswordSideChannel() throws {
        let model = makeModel()

        try model.configure(serverURL: "https://example.com", password: "secret", trustSelfSigned: true)

        let storedRecord = try XCTUnwrap(credentialStore.record)
        XCTAssertEqual(storedRecord.password, "secret")
        XCTAssertEqual(storedRecord.generation, model.credentialGeneration)

        let relaunched = makeModel()
        XCTAssertEqual(relaunched.password, "secret")
        XCTAssertEqual(relaunched.credentialGeneration, model.credentialGeneration)
        XCTAssertEqual(relaunched.serverURLString, "https://example.com")
    }

    func testConfigureFailureDoesNotPartiallyPersistCredentials() {
        let model = makeModel()

        try? model.configure(serverURL: "https://before.example.com", password: "old-secret", trustSelfSigned: false)
        let oldRecord = credentialStore.record
        let oldIdentity = model.configurationIdentity
        credentialStore.failNextSave = true

        XCTAssertThrowsError(try model.configure(serverURL: "https://example.com", password: "secret", trustSelfSigned: true))
        XCTAssertEqual(credentialStore.record, oldRecord)
        XCTAssertEqual(model.password, oldRecord?.password)
        XCTAssertEqual(model.credentialGeneration, oldRecord?.generation)
        XCTAssertEqual(model.serverURLString, "https://before.example.com")
        XCTAssertEqual(model.configurationIdentity, oldIdentity)
        XCTAssertTrue(model.isConfigured)
    }

    func testInitialCredentialMintFailureLeavesAppInertAndUnconfigured() {
        UserDefaults.standard.removeObject(forKey: "phoenix.serverURL")
        credentialStore.failNextSave = true

        let model = makeModel()

        XCTAssertNil(credentialStore.record)
        XCTAssertEqual(model.password, "")
        XCTAssertEqual(model.credentialGeneration, "")
        XCTAssertEqual(model.serverURLString, "")
        let relaunched = makeModel()
        XCTAssertEqual(relaunched.password, "")
        XCTAssertEqual(relaunched.credentialGeneration, "")
        XCTAssertEqual(relaunched.serverURLString, "")
    }

    @MainActor
    func testSignOutResetFencesLateConversationListSaveAndClearsOnlyCurrentBase() async {
        let baseA = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-signout-late-a-\(UUID().uuidString)", isDirectory: true)
        let baseB = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-signout-late-b-\(UUID().uuidString)", isDirectory: true)
        let contextA = DiskStore.versionedContext(baseDirectory: baseA)
        let writerA = contextA.writer(name: ConversationListStore.cacheName, version: ConversationListStore.schemaVersion)
        let storeA = DiskConversationPersistenceStore(baseDirectory: baseA, context: contextA)
        let storeB = DiskConversationPersistenceStore(baseDirectory: baseB)
        DiskStore.baseDirectory = baseA

        let lateList = ConversationListStore(hasCachedSnapshot: { _ in false }, context: contextA)
        let rowA = self.conversation(id: "row-a", aggregateId: "pc-a", title: "late")
        lateList.upsert(rowA)
        let staleRevision = writerA.reserveRevision()

        let pendingA = makePendingOutboxEntry(conversationId: "row-a")
        let pendingB = makePendingOutboxEntry(conversationId: "row-b")
        let outboxHandleA = storeA.outboxPersistence(conversationId: "row-a", aggregateAuthority: "row-a", scope: defaultPersistenceScope)
        let outboxHandleB = storeB.outboxPersistence(conversationId: "row-b", aggregateAuthority: "row-b", scope: defaultPersistenceScope)
        _ = await outboxHandleA.save(PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: "row-a", entries: [pendingA]), revision: outboxHandleA.reserveRevision())
        _ = await outboxHandleB.save(PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: "row-b", entries: [pendingB]), revision: outboxHandleB.reserveRevision())

        let snapshot = ConversationSession.PersistedSnapshot(conversation: nil, messages: [], lastSequenceId: 0, transcriptGeneration: nil, syncedAt: Date(), authoritative: nil)
        let snapshotHandleA = storeA.snapshotPersistence(conversationId: "row-a")
        let snapshotHandleB = storeB.snapshotPersistence(conversationId: "row-b")
        _ = await snapshotHandleA.save(snapshot, revision: snapshotHandleA.reserveRevision())
        _ = await snapshotHandleB.save(snapshot, revision: snapshotHandleB.reserveRevision())

        let model = makeModel(conversationPersistenceStore: storeA)
        await model.signOut()

        _ = await writerA.save(
            ConversationListStore.Cache(
                conversations: [rowA],
                transcriptToAggregate: ["row-a": "pc-a"],
                aggregateToCachedTranscript: ["pc-a": "row-a"],
                lastRefreshed: Date()),
            revision: staleRevision)

        let reloadedA = ConversationListStore(hasCachedSnapshot: { _ in false }, context: contextA)
        XCTAssertTrue(reloadedA.conversations.isEmpty)
        let reloadedB = ConversationListStore(hasCachedSnapshot: { _ in false }, context: DiskStore.versionedContext(baseDirectory: baseB))
        XCTAssertTrue(reloadedB.conversations.isEmpty)
        let pendingAIds = await storeA.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        let pendingBIds = await storeB.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(pendingAIds, [])
        XCTAssertTrue(pendingBIds.contains("row-b"))
    }

    @MainActor
    func testSignOutOnlyRemovesCurrentBasePersistedState() async {
        let baseA = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-signout-a-\(UUID().uuidString)", isDirectory: true)
        let baseB = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-signout-b-\(UUID().uuidString)", isDirectory: true)
        let storeA = DiskConversationPersistenceStore(baseDirectory: baseA)
        let storeB = DiskConversationPersistenceStore(baseDirectory: baseB)

        let pendingA = makePendingOutboxEntry(conversationId: "row-a")
        let pendingB = makePendingOutboxEntry(conversationId: "row-b")
        let outboxHandleA = storeA.outboxPersistence(conversationId: "row-a", aggregateAuthority: "row-a", scope: defaultPersistenceScope)
        let outboxHandleB = storeB.outboxPersistence(conversationId: "row-b", aggregateAuthority: "row-b", scope: defaultPersistenceScope)
        _ = await outboxHandleA.save(PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: "row-a", entries: [pendingA]), revision: outboxHandleA.reserveRevision())
        _ = await outboxHandleB.save(PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: "row-b", entries: [pendingB]), revision: outboxHandleB.reserveRevision())

        let snapshotA = ConversationSession.PersistedSnapshot(conversation: nil, messages: [], lastSequenceId: 0, transcriptGeneration: nil, syncedAt: Date(), authoritative: nil)
        let snapshotB = ConversationSession.PersistedSnapshot(conversation: nil, messages: [], lastSequenceId: 0, transcriptGeneration: nil, syncedAt: Date(), authoritative: nil)
        let snapshotHandleA = storeA.snapshotPersistence(conversationId: "row-a")
        let snapshotHandleB = storeB.snapshotPersistence(conversationId: "row-b")
        _ = await snapshotHandleA.save(snapshotA, revision: snapshotHandleA.reserveRevision())
        _ = await snapshotHandleB.save(snapshotB, revision: snapshotHandleB.reserveRevision())

        let model = makeModel(conversationPersistenceStore: storeA)
        let (api, registration) = makeHTTPAPI(probe: SendProbe())
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        model.replaceAPIForTesting(api)

        await model.signOut()

        let pendingAIds = await storeA.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        let pendingBIds = await storeB.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(pendingAIds, [])
        XCTAssertTrue(pendingBIds.contains("row-b"))
        XCTAssertEqual(storeA.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")), [])
        XCTAssertTrue(storeB.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-b"))
        let snapshotAValue = DiskStore.loadVersionedResult(
            ConversationSession.PersistedSnapshot.self,
            source: baseA.appendingPathComponent("PhoenixMobile", isDirectory: true).appendingPathComponent("conv-row-a").appendingPathExtension("json"),
            version: ConversationSession.snapshotSchemaVersion)
        let snapshotBValue = DiskStore.loadVersionedResult(
            ConversationSession.PersistedSnapshot.self,
            source: baseB.appendingPathComponent("PhoenixMobile", isDirectory: true).appendingPathComponent("conv-row-b").appendingPathExtension("json"),
            version: ConversationSession.snapshotSchemaVersion)
        if case .missing = snapshotAValue {} else { XCTFail("expected base A snapshot removed") }
        if case .value = snapshotBValue {} else { XCTFail("expected base B snapshot retained") }
    }
    @MainActor
    func testSignOutDeletesVersionedAndLegacyCredentialAccounts() async {
        let credentialStore = InMemoryCredentialStore()
        let model = AppModel(credentialStore: credentialStore)

        await model.signOut()

        XCTAssertEqual(
            Set(credentialStore.deletedAccounts),
            ["server-credentials", "server-password"])
    }

    @MainActor
    func testSignOutResetsConfigurationAndEvictsOwnedSessionAndDetailState() async {
        let model = makeModel()
        let (api, registration) = makeHTTPAPI(probe: SendProbe())
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        model.replaceAPIForTesting(api)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let session = model.session(for: "row-1")
        session?.receive(.initSnapshot(.init(
            conversation: self.conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let detail = model.productConversationDetailModel(for: "pc-1", initialTranscriptRowId: "row-1")
        detail.applyForTesting(testProductConversationSnapshot())
        model.pendingOpenConversationId = "pc-1"

        await model.signOut()

        XCTAssertNil(model.configurationIdentity)
        XCTAssertEqual(model.serverURLString, "")
        XCTAssertEqual(model.password, "")
        XCTAssertFalse(model.trustSelfSigned)
        XCTAssertNil(model.existingSession(for: "row-1"))
        XCTAssertNil(model.pendingOpenConversationId)
        XCTAssertNil(model.coordinatorConversationId)
        XCTAssertNil(model.api)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let replacement = model.productConversationDetailModel(for: "pc-1")
        XCTAssertFalse(replacement === detail)
        await model.signOut()
    }

    func testSignOutRevokesDeliverySuspendedBeforePostAndClearsLocalState() async throws {
        let store = MutableTestConversationPersistenceStore(contentsByConversationId: [:])
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.connectivity.setOnlineForTesting(false)
        model.replaceAPIForTesting(api)
        await model.awaitStartupHardDeleteRecoveryForTesting()
        let session = try XCTUnwrap(model.session(for: "row-1"))
        session.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-1"),
            messages: [], agentWorking: false, presentationMode: "idle",
            lastSequenceId: 0, pendingAnchorSequenceId: 0,
            pendingEvents: [], pendingTruncated: false)))
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        let queued = await session.outbox.enqueue(text: "queued")
        XCTAssertNotNil(queued)
        let didPersistQueuedItem = await session.outbox.flushPersistence()
        XCTAssertTrue(didPersistQueuedItem)
        let deliveryGate = AsyncCandidateGate()
        let deliveryCompleted = AsyncCandidateGate()
        store.suspendNextDeliveryPreparation(
            using: deliveryGate,
            completed: deliveryCompleted)
        let cleanupGate = AsyncCandidateGate()
        store.suspendRemoveAll(using: cleanupGate)

        model.connectivity.setOnlineForTesting(true)
        model.foregrounded()
        await deliveryGate.waitForEntry()
        let signOut = Task { await model.signOut() }
        await cleanupGate.waitForEntry()
        XCTAssertEqual(session.hydrationAuthority, .none)

        await deliveryGate.release()
        await deliveryCompleted.waitForEntry()
        XCTAssertTrue(probe.chatPostPaths.isEmpty)
        await cleanupGate.release()
        await signOut.value

        XCTAssertTrue(probe.chatPostPaths.isEmpty)
        XCTAssertTrue(model.listStore.conversations.isEmpty)
        XCTAssertNil(model.existingSession(for: "row-1"))
        XCTAssertTrue(store.inspectOutbox(conversationId: "row-1").visibleEntries.isEmpty)
        XCTAssertFalse(store.hasCachedSnapshot(conversationId: "row-1"))
    }

    func testSignOutRejectsRestoreRefreshAndFencesRefreshAdmittedBeforeReset() async throws {
        let baseDirectory = isolatedDiskDirectory()
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let blocker = DrainBlocker()
        let store = ResettableConversationPersistenceStore(
            baseDirectory: baseDirectory, context: context, resetBlocker: blocker)
        let probe = SendProbe()
        let listBody = try JSONEncoder().encode(ProductConversationListResponse(
            product_conversations: []))
        let (api, registration) = makeHTTPAPI(
            probe: probe, productConversationBody: listBody)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        let oldRow = conversation(id: "row-old", aggregateId: "pc-old")
        model.listStore.upsert(oldRow)
        let admittedBeforeSignOut = model.listStore.externalRefreshToken()

        let signOut = Task { await model.signOut() }
        await blocker.waitForEntry()
        await model.refreshList()

        XCTAssertTrue(probe.listGetPaths.isEmpty)
        XCTAssertFalse(model.listStore.applyExternal([oldRow], startedAt: admittedBeforeSignOut))

        await blocker.release()
        await signOut.value

        XCTAssertTrue(model.listStore.conversations.isEmpty)
        let restored = ConversationListStore(
            hasCachedSnapshot: { _ in false }, context: context)
        XCTAssertTrue(restored.conversations.isEmpty)
    }

    func testSignOutWithSuspendedCleanupDoesNotPublishUnconfiguredStateEarly() async {
        let baseDirectory = isolatedDiskDirectory()
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let blocker = DrainBlocker()
        let store = ResettableConversationPersistenceStore(
            baseDirectory: baseDirectory, context: context, resetBlocker: blocker)
        let credentials = InMemoryCredentialStore()
        try! credentials.saveRecord(
            .init(password: "secret", generation: "old-generation"),
            account: "server-password-record")
        let model = AppModel(
            conversationPersistenceStore: store,
            credentialStore: credentials)
        model.configureForTesting(serverURL: "https://old.example", trustSelfSigned: true)
        model.listStore.upsert(conversation(id: "row-1"))
        let signOut = Task { await model.signOut() }
        await blocker.waitForEntry()

        XCTAssertEqual(model.serverURLString, "https://old.example")
        XCTAssertNotNil(credentials.loadRecord(account: "server-password-record"))
        XCTAssertTrue(model.listStore.conversations.isEmpty)

        await blocker.release()
        await signOut.value

        XCTAssertEqual(model.serverURLString, "")
        XCTAssertNil(credentials.loadRecord(account: "server-password-record"))
        XCTAssertTrue(model.listStore.conversations.isEmpty)
    }

    func testCoordinatorReceiptSurvivesTrustOnlyOfflineRebuild() {
        let identity = APIConfigurationIdentity(
            serverURL: "https://example.com",
            credentialGeneration: "credential",
            trustSelfSigned: false)
        let coordinatorStore = InMemoryCoordinatorIdentityStore(
            "coordinator-row", configurationIdentity: identity)
        let (api, registration) = makeHTTPAPI(
            probe: SendProbe(), configurationIdentity: identity)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = AppModel(
            coordinatorIdentityStore: coordinatorStore,
            credentialStore: InMemoryCredentialStore())
        model.replaceAPIForTesting(api)

        model.rebuildTrustForTesting(true)

        XCTAssertEqual(model.coordinatorConversationId, "coordinator-row")
        XCTAssertEqual(coordinatorStore.receiptsByPersistenceScope.count, 1)
    }

    func testPersistedMemberDiscoveryIncludesVisibleFailedV2OutboxWithoutMakingItDrainable() async {
        let baseDirectory = isolatedDiskDirectory()
        let store = DiskConversationPersistenceStore(
            baseDirectory: baseDirectory,
            context: DiskStore.versionedContext(baseDirectory: baseDirectory))
        var failed = makePendingOutboxEntry(conversationId: "row-failed")
        failed.status = .failed
        failed.lastError = "server rejected request"
        let handle = store.outboxPersistence(
            conversationId: "row-failed",
            aggregateAuthority: "pc-1",
            scope: defaultPersistenceScope)
        let saved = await handle.save(
            PersistedOutboxEnvelope(
                scope: defaultPersistenceScope,
                aggregateAuthority: "pc-1",
                entries: [failed]),
            revision: handle.reserveRevision())
        XCTAssertTrue(saved)

        let discovery = await store.persistedMemberDiscovery(
            aggregateId: "pc-1", scope: defaultPersistenceScope)
        let drainOwners = await store.pendingOutboxOwners(scope: defaultPersistenceScope)

        XCTAssertEqual(discovery.persistedOutboxOwnerIds, ["row-failed"])
        XCTAssertFalse(drainOwners.contains("row-failed"))
    }

    func testLegacyCoordinatorIdentityImportsOnceIntoCurrentPersistenceScope() {
        let key = "phoenix.coordinatorConversationId"
        UserDefaults.standard.set("legacy-coordinator", forKey: key)
        defer { UserDefaults.standard.removeObject(forKey: key) }
        let serverKey = "phoenix.serverURL"
        let trustKey = "phoenix.trustSelfSigned"
        UserDefaults.standard.set("https://example.com", forKey: serverKey)
        UserDefaults.standard.set(false, forKey: trustKey)
        defer {
            UserDefaults.standard.removeObject(forKey: serverKey)
            UserDefaults.standard.removeObject(forKey: trustKey)
        }
        let credentialStore = InMemoryCredentialStore()
        try! credentialStore.saveRecord(
            .init(password: "secret", generation: "credential"),
            account: "server-credentials")
        let identity = APIConfigurationIdentity(
            serverURL: "https://example.com",
            credentialGeneration: "credential",
            trustSelfSigned: false)
        let coordinatorStore = InMemoryCoordinatorIdentityStore()

        let model = AppModel(
            coordinatorIdentityStore: coordinatorStore,
            credentialStore: credentialStore)

        XCTAssertEqual(model.coordinatorConversationId, "legacy-coordinator")
        XCTAssertNil(UserDefaults.standard.string(forKey: key))
        XCTAssertEqual(
            coordinatorStore.receiptsByPersistenceScope[identity.persistenceScope]?.conversationId,
            "legacy-coordinator")

        UserDefaults.standard.set("different-legacy", forKey: key)
        let reloaded = AppModel(
            coordinatorIdentityStore: coordinatorStore,
            credentialStore: credentialStore)
        XCTAssertEqual(reloaded.coordinatorConversationId, "legacy-coordinator")
        XCTAssertEqual(UserDefaults.standard.string(forKey: key), "different-legacy")
    }

    func testSignOutClearsInjectedCoordinatorIdentityStore() async {
        let (api, registration) = makeHTTPAPI(probe: SendProbe())
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let identityStore = InMemoryCoordinatorIdentityStore(
            "coordinator-row",
            configurationIdentity: api.configurationIdentity)
        let model = makeModel(coordinatorIdentityStore: identityStore)
        model.replaceAPIForTesting(api)

        await model.signOut()

        XCTAssertNil(identityStore.receiptsByPersistenceScope[api.configurationIdentity.persistenceScope])
        XCTAssertNil(model.coordinatorConversationId)
    }

    func testConfigureStoresCredentialGenerationAsRandomBytesAndSignOutClearsIt() async {
        let model = makeModel()

        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)

        let credentialRecord = credentialStore.record
        XCTAssertEqual(credentialRecord?.generation, model.credentialGeneration)
        XCTAssertEqual(credentialRecord?.password, model.password)
        XCTAssertEqual(model.credentialGeneration.count, 32)

        await model.signOut()

        XCTAssertNil(credentialStore.record)
    }

    func testConfigureRotatesCredentialGenerationAndPersistsOnlyCurrentBytes() {
        let model = makeModel()

        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let firstGeneration = model.credentialGeneration
        let firstRecord = credentialStore.record

        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: false)
        let secondGeneration = model.credentialGeneration
        let secondRecord = credentialStore.record

        XCTAssertNotEqual(firstGeneration, secondGeneration)
        XCTAssertEqual(firstRecord?.generation, firstGeneration)
        XCTAssertEqual(secondRecord?.generation, secondGeneration)
        XCTAssertNotEqual(firstRecord?.generation, secondRecord?.generation)
    }

    func testConfigurationIdentityUsesCurrentCredentialGeneration() {
        let model = makeModel()

        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let firstGeneration = model.credentialGeneration
        let firstIdentity = model.configurationIdentity
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: false)
        let secondGeneration = model.credentialGeneration
        let secondIdentity = model.configurationIdentity

        XCTAssertNotEqual(firstGeneration, secondGeneration)
        XCTAssertEqual(firstIdentity?.credentialGeneration, firstGeneration)
        XCTAssertEqual(secondIdentity?.credentialGeneration, secondGeneration)
    }

    func testParallelModelInstancesDoNotInterfereThroughCoordinatorIdentity() async {
        let firstIdentity = APIConfigurationIdentity(serverURL: "https://a.example.com", credentialGeneration: "a-gen", trustSelfSigned: true)
        let secondIdentity = APIConfigurationIdentity(serverURL: "https://b.example.com", credentialGeneration: "b-gen", trustSelfSigned: true)
        let firstStore = InMemoryCoordinatorIdentityStore("coordinator-a", configurationIdentity: firstIdentity)
        let secondStore = InMemoryCoordinatorIdentityStore("coordinator-b", configurationIdentity: secondIdentity)
        let first = makeModel(coordinatorIdentityStore: firstStore)
        let second = makeModel(coordinatorIdentityStore: secondStore)
        first.replaceAPIForTesting(PhoenixAPI(baseURL: URL(string: firstIdentity.serverURL)!, password: nil, allowSelfSigned: true, configurationIdentity: firstIdentity)!)
        second.replaceAPIForTesting(PhoenixAPI(baseURL: URL(string: secondIdentity.serverURL)!, password: nil, allowSelfSigned: true, configurationIdentity: secondIdentity)!)

        await first.signOut()

        XCTAssertTrue(firstStore.receiptsByPersistenceScope.isEmpty)
        XCTAssertEqual(secondStore.receiptsByPersistenceScope[secondIdentity.persistenceScope]?.conversationId, "coordinator-b")
        XCTAssertEqual(second.coordinatorConversationId, "coordinator-b")
    }

    func testRebuildAPIKeepsCurrentConfigurationCoordinatorReceipt() {
        let identityStore = InMemoryCoordinatorIdentityStore()
        let model = makeModel(coordinatorIdentityStore: identityStore)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let identity = model.configurationIdentity!
        identityStore.receiptsByPersistenceScope[identity.persistenceScope] = CoordinatorIdentityReceipt(persistenceScope: identity.persistenceScope, conversationId: "coordinator-row")
        model.replaceAPIForTesting(PhoenixAPI(baseURL: URL(string: identity.serverURL)!, password: nil, allowSelfSigned: identity.trustSelfSigned, configurationIdentity: identity)!)

        XCTAssertEqual(model.coordinatorConversationId, "coordinator-row")
        XCTAssertEqual(identityStore.receiptsByPersistenceScope[identity.persistenceScope]?.conversationId, "coordinator-row")
    }

    func testDiskConversationPersistenceStoreRejectsForeignAndMalformedEntries() {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-outbox-store-tests-\(UUID().uuidString)", isDirectory: true)
        let phoenixDirectory = baseDirectory.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try? FileManager.default.createDirectory(at: phoenixDirectory, withIntermediateDirectories: true)
        let valid = makePendingOutboxEntry(conversationId: "row-1")
        let foreign = makePendingOutboxEntry(conversationId: "row-2")
        let validData = try! JSONEncoder().encode(TestDiskEnvelope(schema_version: Outbox.schemaVersion, payload: PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: "row-1", entries: [valid, foreign])))
        try! validData.write(to: phoenixDirectory.appendingPathComponent("outbox-row-1.json"), options: .atomic)
        try! Data("{bad".utf8).write(to: phoenixDirectory.appendingPathComponent("outbox-.json"), options: .atomic)

        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory)

        XCTAssertTrue(store.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
        XCTAssertEqual(
            store.inspectOutbox(conversationId: "row-1").visibleEntries.map(\.conversationId),
            ["row-1"])
    }

    func testDiskConversationPersistenceStorePendingOutboxOwnerTranscriptRowIdsUsesCapturedBaseAndFiltersPendingVisible() async {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-outbox-candidates-\(UUID().uuidString)", isDirectory: true)
        let phoenixDirectory = baseDirectory.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try? FileManager.default.createDirectory(at: phoenixDirectory, withIntermediateDirectories: true)
        let pending = makePendingOutboxEntry(conversationId: "row-1")
        let accepted = OutboxEntry(
            localId: UUID().uuidString.lowercased(),
            conversationId: "row-2",
            text: "accepted",
            images: [],
            status: .pending,
            acceptedByServer: true,
            createdAt: Date(),
            acceptedAt: Date(),
            lastError: nil,
            attemptCount: 1)
        let hidden = OutboxEntry(
            localId: UUID().uuidString.lowercased(),
            conversationId: "row-3",
            text: "hidden",
            images: [],
            status: .reconciled,
            acceptedByServer: false,
            createdAt: Date(),
            acceptedAt: nil,
            lastError: nil,
            attemptCount: 1)
        let otherBase = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-outbox-candidates-other-\(UUID().uuidString)", isDirectory: true)
        let otherPhoenixDirectory = otherBase.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try? FileManager.default.createDirectory(at: otherPhoenixDirectory, withIntermediateDirectories: true)
        let external = makePendingOutboxEntry(conversationId: "row-external")

        func writeEntries(_ entries: [OutboxEntry], conversationId: String, directory: URL) {
            let data = try! JSONEncoder().encode(TestDiskEnvelope(schema_version: Outbox.schemaVersion, payload: PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: conversationId, entries: entries)))
            try! data.write(to: directory.appendingPathComponent("outbox-\(conversationId).json"), options: .atomic)
        }

        writeEntries([pending], conversationId: "row-1", directory: phoenixDirectory)
        writeEntries([accepted], conversationId: "row-2", directory: phoenixDirectory)
        writeEntries([hidden], conversationId: "row-3", directory: phoenixDirectory)
        writeEntries([external], conversationId: "row-external", directory: otherPhoenixDirectory)

        let storeA = DiskConversationPersistenceStore(baseDirectory: baseDirectory)
        let storeB = DiskConversationPersistenceStore(baseDirectory: otherBase)
        DiskStore.baseDirectory = otherBase

        let ids = await storeA.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))

        XCTAssertTrue(ids.contains("row-1"))
        let storeBPendingIds = await storeB.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertTrue(storeBPendingIds.contains("row-external"))
        if case .accessible(_, _, let entries) = storeA.inspectOutbox(conversationId: "row-1").state {
            XCTAssertEqual(entries.map(\.conversationId), ["row-1"])
        } else {
            XCTFail("expected row-1 entries from store A")
        }
        if case .accessible(_, _, let entries) = storeB.inspectOutbox(conversationId: "row-external").state {
            XCTAssertEqual(entries.map(\.conversationId), ["row-external"])
        } else {
            XCTFail("expected row-external entries from store B")
        }

        let handleA = OutboxPersistenceHandle.disk(conversationId: "row-1", baseDirectory: baseDirectory)
        let handleB = OutboxPersistenceHandle.disk(conversationId: "row-1", baseDirectory: otherBase)
        let baseBPending = makePendingOutboxEntry(conversationId: "row-1")
        _ = await handleB.save(PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: "row-1", entries: [baseBPending]), revision: handleB.reserveRevision())
        await handleA.remove(revision: handleA.reserveRevision())

        let storeBPendingIdsAfterHandleARemoval = await storeB.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(Set(storeBPendingIdsAfterHandleARemoval.map(\.transcriptRowId)), ["row-external", "row-1"])
    }

    func testDiskConversationPersistenceStoreRemoveAllPersistsInstanceIsolation() async {
        let baseA = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-outbox-remove-all-a-\(UUID().uuidString)", isDirectory: true)
        let baseB = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-outbox-remove-all-b-\(UUID().uuidString)", isDirectory: true)
        let phoenixA = baseA.appendingPathComponent("PhoenixMobile", isDirectory: true)
        let phoenixB = baseB.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try? FileManager.default.createDirectory(at: phoenixA, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: phoenixB, withIntermediateDirectories: true)

        let pendingA = makePendingOutboxEntry(conversationId: "row-a")
        let pendingB = makePendingOutboxEntry(conversationId: "row-b")
        let snapshotA = ConversationSession.PersistedSnapshot(conversation: nil, messages: [], lastSequenceId: 0, transcriptGeneration: nil, syncedAt: Date(), authoritative: nil)
        let snapshotB = ConversationSession.PersistedSnapshot(conversation: nil, messages: [], lastSequenceId: 0, transcriptGeneration: nil, syncedAt: Date(), authoritative: nil)
        let outboxA = try! JSONEncoder().encode(TestDiskEnvelope(schema_version: Outbox.schemaVersion, payload: PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: "row-a", entries: [pendingA])))
        let outboxB = try! JSONEncoder().encode(TestDiskEnvelope(schema_version: Outbox.schemaVersion, payload: PersistedOutboxEnvelope(scope: defaultPersistenceScope, aggregateAuthority: "row-b", entries: [pendingB])))
        let convA = try! JSONEncoder().encode(TestDiskEnvelope(schema_version: ConversationSession.snapshotSchemaVersion, payload: snapshotA))
        let convB = try! JSONEncoder().encode(TestDiskEnvelope(schema_version: ConversationSession.snapshotSchemaVersion, payload: snapshotB))
        try! outboxA.write(to: phoenixA.appendingPathComponent("outbox-row-a.json"), options: Data.WritingOptions.atomic)
        try! outboxB.write(to: phoenixB.appendingPathComponent("outbox-row-b.json"), options: Data.WritingOptions.atomic)
        try! convA.write(to: phoenixA.appendingPathComponent("conv-row-a.json"), options: Data.WritingOptions.atomic)
        try! convB.write(to: phoenixB.appendingPathComponent("conv-row-b.json"), options: Data.WritingOptions.atomic)

        let storeA = DiskConversationPersistenceStore(baseDirectory: baseA)
        let storeB = DiskConversationPersistenceStore(baseDirectory: baseB)

        await storeA.removeAllPersistedConversationState()

        let storeAPendingIds = await storeA.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(storeAPendingIds, [])
        let storeBPendingIds = await storeB.pendingOutboxOwners(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertTrue(storeBPendingIds.contains("row-b"))
        if case .missing = storeA.inspectOutbox(conversationId: "row-a").state {
        } else {
            XCTFail("expected store A outbox removed")
        }
        if case .accessible(_, _, let entries) = storeB.inspectOutbox(conversationId: "row-b").state {
            XCTAssertEqual(entries.map(\.conversationId), ["row-b"])
        } else {
            XCTFail("expected store B outbox retained")
        }
    }

    func testReconfigurationInvalidatesCachedDetailModel() {
        let first = makeModel()
        first.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let detailA = first.productConversationDetailModel(for: "pc-1")

        first.configureForTesting(serverURL: "https://example.org", trustSelfSigned: true)
        let detailB = first.productConversationDetailModel(for: "pc-1")

        XCTAssertFalse(detailA === detailB)
    }


    @MainActor
    func testAggregateInit404ClearsLegacyOutboxWithoutSend() async {
        _ = isolatedDiskDirectory()
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            snapshotsByConversationId: ["row-1"],
            aggregateMembersById: ["pc-1": ["row-1"]])
        let probe = SendProbe()
        let host = "appmodel-send-4.invalid"
        let body = Data("{\"error\":\"not found\"}".utf8)
        let (api, registration) = makeHTTPAPI(
            probe: probe,
            host: host,
            productConversationStatusCode: 404,
            productConversationBody: body)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        let detail = model.productConversationDetailModel(for: "pc-1", initialTranscriptRowId: "row-1")

        await detail.start()
        await detail.awaitCurrentLoadForTesting()

        XCTAssertEqual(probe.aggregateGetPaths.count, 1)
        XCTAssertEqual(probe.chatPostPaths.count, 0)
        if case .missing = store.inspectOutbox(conversationId: "row-1").state {
        } else {
            XCTFail("expected typed 404 cleanup to clear durable outbox")
        }
        XCTAssertFalse(store.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
        XCTAssertNil(model.existingSession(for: "row-1"))
        let replacement = model.productConversationDetailModel(for: "pc-1")
        XCTAssertFalse(replacement === detail)
    }
    @MainActor
    func testTranscriptHardDeleteMatchesAggregateCleanupForPersistedStateAndBlockedLateDrain() async {
        _ = isolatedDiskDirectory()
        let blocker = DrainBlocker()
        let probe = SendProbe()
        let host = "hard-delete-sse.invalid"
        let body = Data("{}".utf8)
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1", "row-0"],
            contentsByConversationId: [
                "row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")]),
                "row-0": .entries([makePendingOutboxEntry(conversationId: "row-0")]),
            ],
            snapshotsByConversationId: ["row-1", "row-0"],
            aggregateMembersById: ["pc-1": ["row-0", "row-1"]])
        store.onPendingOutboxOwnerTranscriptRowIds = { await blocker.block(); return ["row-1"] }
        let model = makeModel(conversationPersistenceStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let registration = TestURLProtocol.install(host: host) { request in
            probe.record(request)
            let url = request.url!
            if request.httpMethod == "GET", url.path.contains("/api/product-conversations/") {
                let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
                return (response, body)
            }
            if request.httpMethod == "POST", url.path.contains("/chat") {
                let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
                return (response, Data(#"{"queued":false}"#.utf8))
            }
            let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
            return (response, Data("{}".utf8))
        }
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        model.replaceAPIForTesting(PhoenixAPI(
            baseURL: URL(string: "https://\(host)")!,
            password: nil,
            allowSelfSigned: false,
            configurationIdentity: APIConfigurationIdentity(
                serverURL: "https://\(host)",
                credentialGeneration: model.credentialGeneration,
                trustSelfSigned: false),
            session: URLSession(configuration: {
                let configuration = URLSessionConfiguration.ephemeral
                configuration.protocolClasses = [TestURLProtocol.self]
                return configuration
            }()),
            streamSession: URLSession(configuration: {
                let configuration = URLSessionConfiguration.ephemeral
                configuration.protocolClasses = [TestURLProtocol.self]
                return configuration
            }()))!)
        model.listStore.upsert(self.conversation(id: "row-1", aggregateId: "pc-1"))
        model.listStore.upsert(self.conversation(id: "row-0", aggregateId: "pc-1"))
        let session = model.session(for: "row-1")
        session?.receive(.initSnapshot(.init(
            conversation: self.conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let sibling = model.session(for: "row-0")
        sibling?.receive(.initSnapshot(.init(
            conversation: self.conversation(id: "row-0", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let detail = model.productConversationDetailModel(for: "pc-1", initialTranscriptRowId: "row-1")
        detail.applyForTesting(testProductConversationSnapshot())

        model.triggerPersistedOutboxDrainIfNeededForTesting()
        let drainGeneration = try! XCTUnwrap(model.currentPersistedOutboxDrainGenerationForTesting())
        await blocker.waitForEntry()
        let chatPostsBeforeDelete = probe.chatPostPaths
        session?.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await model.awaitHardDeleteCleanupForTesting(conversationId: "row-1")
        await blocker.release()
        _ = await model.awaitPersistedOutboxDrainForTesting(generation: drainGeneration)

        XCTAssertEqual(probe.chatPostPaths, chatPostsBeforeDelete)
        if case .missing = store.inspectOutbox(conversationId: "row-1").state {} else { XCTFail("expected deleted row state removed") }
        if case .missing = store.inspectOutbox(conversationId: "row-0").state {} else { XCTFail("expected sibling row state removed") }
        XCTAssertFalse(store.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
        XCTAssertFalse(store.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-0"))
        XCTAssertNil(model.existingSession(for: "row-1"))
        XCTAssertNil(model.existingSession(for: "row-0"))
        let replacement = model.productConversationDetailModel(for: "pc-1")
        XCTAssertFalse(replacement === detail)
    }

    @MainActor
    func testSessionHardDeleteRemovesCurrentScopeOutboxOnlyMember() async {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1", "row-outbox"],
            contentsByConversationId: [
                "row-1": .entries([]),
                "row-outbox": .entries([makePendingOutboxEntry(conversationId: "row-outbox")]),
            ])
        store.persistedMemberDiscoveryOverride = .init(
            currentAuthorityMemberIds: [],
            persistedOutboxOwnerIds: ["row-outbox"])
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        let session = try! XCTUnwrap(model.session(for: "row-1", aggregateAuthority: "pc-1"))
        session.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [], agentWorking: false, presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await session.awaitHardDeleteReportForTesting()
        await model.awaitHardDeleteCleanupForTesting(conversationId: "row-1")

        if case .missing = store.inspectOutbox(conversationId: "row-outbox").state {
        } else {
            XCTFail("expected outbox-only member removal")
        }
    }

    func testTrustReplacementRecoversFenceCommittedByStaleEpoch() async throws {
        let gate = AsyncCandidateGate()
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            aggregateMembersById: ["pc-1": ["row-1"]])
        store.persistHardDeleteFenceGate = gate
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.connectivity.setOnlineForTesting(false)
        model.replaceAPIForTesting(api)
        let session = try XCTUnwrap(model.session(for: "row-1", aggregateAuthority: "pc-1"))
        session.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"), messages: [],
            agentWorking: false, presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await gate.waitForEntry()
        model.rebuildTrustForTesting(true)
        await gate.release()
        await session.awaitHardDeleteReportForTesting()
        await model.awaitStartupHardDeleteRecoveryForTesting()

        XCTAssertTrue(store.persistedHardDeleteFences.isEmpty)
        XCTAssertTrue(probe.chatPostPaths.isEmpty)
        XCTAssertNil(model.session(for: "row-1", aggregateAuthority: "pc-1"))
    }

    func testConcurrentAggregateHardDeletesCommitOneFence() async throws {
        let gate = AsyncCandidateGate()
        let discoveryGate = AsyncCandidateGate()
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1", "row-2"],
            contentsByConversationId: ["row-1": .entries([]), "row-2": .entries([])],
            aggregateMembersById: ["pc-1": ["row-1"]])
        store.persistHardDeleteFenceGate = gate
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.connectivity.setOnlineForTesting(false)
        model.replaceAPIForTesting(api)
        let first = try XCTUnwrap(model.session(for: "row-1", aggregateAuthority: "pc-1"))
        let second = try XCTUnwrap(model.session(for: "row-2", aggregateAuthority: "pc-1"))
        first.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"), messages: [],
            agentWorking: false, presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        first.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await gate.waitForEntry()
        store.aggregateMembersById["pc-1"] = ["row-1", "row-2"]
        store.persistedMemberDiscoveryGate = discoveryGate
        second.receive(.conversationHardDeleted(seq: 1, conversationId: "row-2"))
        await discoveryGate.waitForEntry()
        XCTAssertEqual(store.hardDeleteFencePersistAttemptCount, 1)

        await discoveryGate.release()
        await second.awaitHardDeleteReportForTesting()
        await gate.release()
        await first.awaitHardDeleteReportForTesting()
        XCTAssertEqual(store.hardDeleteFencePersistAttemptCount, 2)
        XCTAssertEqual(
            Set(try XCTUnwrap(store.persistedHardDeleteFenceHistory.last).memberConversationIds),
            ["row-1", "row-2"])
    }

    func testHardDeleteFenceFailureLeavesAuthoritativeStateAndOutboxIntact() async throws {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            aggregateMembersById: ["pc-1": ["row-1"]])
        store.persistHardDeleteFenceResult = false
        let model = makeModel(conversationPersistenceStore: store)
        let (api, registration) = makeHTTPAPI(probe: SendProbe())
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        model.replaceAPIForTesting(api)
        model.listStore.upsert(conversation(id: "row-1", aggregateId: "pc-1"))
        let session = try XCTUnwrap(model.session(for: "row-1", aggregateAuthority: "pc-1"))
        session.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [], agentWorking: false, presentationMode: "idle",
            lastSequenceId: 0, pendingAnchorSequenceId: 0,
            pendingEvents: [], pendingTruncated: false)))

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await session.awaitHardDeleteReportForTesting()
        await model.awaitHardDeleteCleanupForTesting(conversationId: "row-1")

        let retainedSession = try XCTUnwrap(model.existingSession(for: "row-1"))
        XCTAssertFalse(retainedSession.acceptsConversationActions)
        let acceptedWhilePending = await retainedSession.send(
            text: "must not enqueue while fence is uncommitted")
        XCTAssertFalse(acceptedWhilePending)
        XCTAssertFalse(model.listStore.conversations.isEmpty)
        XCTAssertFalse(store.inspectOutbox(conversationId: "row-1").visibleEntries.isEmpty)
        XCTAssertNil(model.session(for: "row-1", aggregateAuthority: "pc-1"))
    }

    @MainActor
    func testFailedInitialHardDeleteFenceSaveRetriesBeforeOutboxDrain() async throws {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")])],
            aggregateMembersById: ["pc-1": ["row-1"]])
        store.persistHardDeleteFenceResult = false
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.connectivity.setOnlineForTesting(false)
        model.replaceAPIForTesting(api)
        model.listStore.upsert(conversation(id: "row-1", aggregateId: "pc-1"))
        let session = try XCTUnwrap(model.session(for: "row-1", aggregateAuthority: "pc-1"))
        session.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [], agentWorking: false, presentationMode: "idle",
            lastSequenceId: 0, pendingAnchorSequenceId: 0,
            pendingEvents: [], pendingTruncated: false)))

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await session.awaitHardDeleteReportForTesting()
        model.triggerPersistedOutboxDrainIfNeededForTesting()
        XCTAssertTrue(probe.chatPostPaths.isEmpty)
        XCTAssertNil(model.session(for: "row-1", aggregateAuthority: "pc-1"))

        store.persistHardDeleteFenceResult = true
        model.triggerStartupHardDeleteRecoveryForTesting()
        await model.awaitStartupHardDeleteRecoveryForTesting()
        model.connectivity.setOnlineForTesting(true)

        XCTAssertEqual(store.persistedHardDeleteFenceHistory.map(\.aggregateAuthority), ["pc-1"])
        XCTAssertTrue(probe.chatPostPaths.isEmpty)
        if case .missing = store.inspectOutbox(conversationId: "row-1").state {} else { XCTFail("expected retried fence cleanup to remove outbox") }
        XCTAssertTrue(model.listStore.conversations.isEmpty)
    }

    func testHardDeleteClearsRetainedProductConversationProjection() async {
        let store = MutableTestConversationPersistenceStore(
            contentsByConversationId: ["row-1": .entries([])],
            aggregateMembersById: ["pc-1": ["row-1"]])
        let model = makeModel(conversationPersistenceStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let detail = model.productConversationDetailModel(for: "pc-1")
        detail.applyForTesting(testProductConversationSnapshot())
        XCTAssertFalse(detail.transcriptItems.isEmpty)
        XCTAssertNotNil(detail.actionSession)
        model.pendingOpenConversationId = "row-0"

        await model.forceAggregateNotFoundCleanupForTesting(
            aggregateId: "pc-1",
            transcriptRowId: "row-1",
            memberIds: ["row-0", "row-1"])

        XCTAssertNil(model.pendingOpenConversationId)
        XCTAssertNil(detail.snapshot)
        XCTAssertTrue(detail.transcriptItems.isEmpty)
        XCTAssertNil(detail.actionSession)
        XCTAssertNil(detail.writableTranscriptRowId)
        XCTAssertFalse(detail.canSendChat)
    }

    func testSessionHardDeleteBeforeInitStillCommitsFenceAndRemovesState() async {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-pre-init-delete-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        let model = makeModel(conversationPersistenceStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let session = try! XCTUnwrap(model.session(for: "row-1"))

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await session.awaitHardDeleteReportForTesting()
        await model.awaitHardDeleteCleanupForTesting(conversationId: "row-1")

        XCTAssertTrue(store.hardDeleteFences(persistenceScope: APIConfigurationIdentity(
                serverURL: "https://example.com",
                credentialGeneration: "test-default",
                trustSelfSigned: true).persistenceScope) == .accessible([]))
        XCTAssertNil(model.existingSession(for: "row-1"))
    }

    func testHardDeleteFenceStorageIdentitySeparatesConfigurationsAndRetiresExactPath() async {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-fence-identity-\(UUID().uuidString)")
        let store = DiskConversationPersistenceStore(
            baseDirectory: baseDirectory,
            context: DiskStore.versionedContext(baseDirectory: baseDirectory))
        let identityA = defaultConfigurationIdentity
        let identityB = APIConfigurationIdentity(
            serverURL: identityA.serverURL,
            credentialGeneration: "other-credential",
            trustSelfSigned: identityA.trustSelfSigned)
        let fenceA = PersistedHardDeleteFence(
            persistenceScope: identityA.persistenceScope,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])
        let fenceB = PersistedHardDeleteFence(
            persistenceScope: identityB.persistenceScope,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])

        XCTAssertNotEqual(fenceA.storageName, fenceB.storageName)
        let savedA = await store.replaceHardDeleteFence(expected: nil, replacement: fenceA)
        let savedB = await store.replaceHardDeleteFence(expected: nil, replacement: fenceB)
        XCTAssertEqual(savedA, .replaced)
        XCTAssertEqual(savedB, .replaced)
        let retiredA = await store.retireHardDeleteFence(expected: fenceA)
        XCTAssertEqual(retiredA, .replaced)

        XCTAssertTrue(store.hardDeleteFences(persistenceScope: identityA.persistenceScope) == .accessible([]))
        XCTAssertEqual(store.hardDeleteFences(persistenceScope: identityB.persistenceScope), .accessible([fenceB]))
    }

    func testStartupRecoversDurableHardDeleteFenceBeforeOutboxDrain() async {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-hard-delete-recovery-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let identity = api.configurationIdentity
        let entry = makePendingOutboxEntry(conversationId: "row-1")
        let outbox = store.outboxPersistence(
            conversationId: "row-1",
            aggregateAuthority: "pc-1",
            scope: identity.persistenceScope)
        _ = await outbox.save(
            PersistedOutboxEnvelope(
                scope: identity.persistenceScope,
                aggregateAuthority: "pc-1",
                entries: [entry]),
            revision: outbox.reserveRevision())
        let snapshot = ConversationSession.PersistedSnapshot(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [], lastSequenceId: 0, transcriptGeneration: nil, syncedAt: Date(),
            authoritative: .init(
                configurationIdentity: identity,
                aggregateAuthority: "pc-1",
                syncedAt: Date()))
        let snapshotWriter = store.snapshotPersistence(conversationId: "row-1")
        _ = await snapshotWriter.save(snapshot, revision: snapshotWriter.reserveRevision())
        let fence = PersistedHardDeleteFence(
            persistenceScope: identity.persistenceScope,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])
        _ = await store.replaceHardDeleteFence(expected: nil, replacement: fence)

        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        model.connectivity.setOnlineForTesting(true)
        model.triggerStartupHardDeleteRecoveryForTesting()
        await model.awaitStartupHardDeleteRecoveryForTesting()

        XCTAssertTrue(probe.chatPostPaths.isEmpty)
        XCTAssertNil(model.session(for: "row-1"))
        let directory = DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent("conv-row-1.json").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent("outbox-row-1.json").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent(fence.storageName).appendingPathExtension("json").path))
    }

    func testAuthoritativeCleanupFencesReservedUnpublishedSnapshotAndOutboxSaves() async {
        let baseDirectory = isolatedDiskDirectory()
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        let snapshotWriter = store.snapshotPersistence(conversationId: "row-1")
        let outbox = store.outboxPersistence(
            conversationId: "row-1",
            aggregateAuthority: "pc-1",
            scope: defaultPersistenceScope)
        let staleSnapshotRevision = snapshotWriter.reserveRevision()
        let staleOutboxRevision = outbox.reserveRevision()
        let now = Date()
        let snapshot = ConversationSession.PersistedSnapshot(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [],
            lastSequenceId: 0,
            transcriptGeneration: 1,
            syncedAt: now,
            authoritative: .init(
                configurationIdentity: defaultConfigurationIdentity,
                aggregateAuthority: "pc-1",
                syncedAt: now))
        let envelope = PersistedOutboxEnvelope(
            scope: defaultPersistenceScope,
            aggregateAuthority: "pc-1",
            entries: [])

        let removed = await store.removeAuthoritativePersistedConversationState(
            conversationId: "row-1",
            configurationIdentity: defaultConfigurationIdentity,
            aggregateAuthority: "pc-1")
        let staleSnapshotCompleted = await snapshotWriter.save(
            snapshot, revision: staleSnapshotRevision)
        let staleOutboxCompleted = await outbox.save(
            envelope, revision: staleOutboxRevision)

        XCTAssertTrue(removed)
        XCTAssertTrue(staleSnapshotCompleted)
        XCTAssertTrue(staleOutboxCompleted)
        XCTAssertFalse(store.hasCachedSnapshot(conversationId: "row-1"))
        XCTAssertEqual(store.inspectOutbox(conversationId: "row-1").state, .missing)
    }

    func testHardDeleteCleanupRemovesProvenLegacySnapshotButPreservesForeignUnscopedSnapshot() async throws {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-legacy-cleanup-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        let identity = APIConfigurationIdentity(
            serverURL: "https://phoenix.invalid",
            credentialGeneration: "legacy-installation",
            trustSelfSigned: false)
        let directory = DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
        for (row, aggregate) in [("row-proven", "pc-1"), ("row-foreign", "pc-other")] {
            let writer = context.writer(
                destinationURL: directory.appendingPathComponent("conv-\(row).json"),
                version: ConversationSession.snapshotSchemaVersion)
            _ = await writer.save(
                ConversationSession.PersistedSnapshot(
                    conversation: conversation(id: row, aggregateId: aggregate),
                    messages: [],
                    lastSequenceId: 0,
                    transcriptGeneration: nil,
                    syncedAt: Date(),
                    authoritative: nil),
                revision: writer.reserveRevision())
        }

        let removed = await store.removeAuthoritativePersistedConversationState(
            conversationId: "row-proven",
            configurationIdentity: identity,
            aggregateAuthority: "pc-1",
            legacyScope: identity.persistenceScope)
        let foreignSnapshotIgnored = await store.removeAuthoritativePersistedConversationState(
            conversationId: "row-foreign",
            configurationIdentity: identity,
            aggregateAuthority: "pc-1",
            legacyScope: identity.persistenceScope)

        XCTAssertTrue(removed)
        XCTAssertTrue(foreignSnapshotIgnored)
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("conv-row-proven.json").path))
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("conv-row-foreign.json").path))
    }

    func testStartupRecoversFenceAfterTrustToggleBeforeDrain() async {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-hard-delete-trust-toggle-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        let identityBeforeCrash = APIConfigurationIdentity(
            serverURL: "https://phoenix.invalid",
            credentialGeneration: "same-credential",
            trustSelfSigned: false)
        let fence = PersistedHardDeleteFence(
            persistenceScope: identityBeforeCrash.persistenceScope,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])
        _ = await store.replaceHardDeleteFence(expected: nil, replacement: fence)
        let entry = makePendingOutboxEntry(conversationId: "row-1")
        let outbox = store.outboxPersistence(
            conversationId: "row-1",
            aggregateAuthority: "pc-1",
            scope: identityBeforeCrash.persistenceScope)
        _ = await outbox.save(
            .init(scope: identityBeforeCrash.persistenceScope, aggregateAuthority: "pc-1", entries: [entry]),
            revision: outbox.reserveRevision())
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(
            probe: probe,
            configurationIdentity: .init(
                serverURL: "https://phoenix.invalid",
                credentialGeneration: "same-credential",
                trustSelfSigned: true))
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        model.connectivity.setOnlineForTesting(true)
        model.triggerStartupHardDeleteRecoveryForTesting()
        await model.awaitStartupHardDeleteRecoveryForTesting()

        XCTAssertTrue(probe.chatPostPaths.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent(fence.storageName).appendingPathExtension("json").path))
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("outbox-row-1.json").path))
    }

    func testStartupRecoversPartialHardDeleteAndRetiresFence() async {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-hard-delete-partial-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        let (api, registration) = makeHTTPAPI(probe: SendProbe())
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let identity = api.configurationIdentity
        let entry = makePendingOutboxEntry(conversationId: "row-1")
        let outbox = store.outboxPersistence(
            conversationId: "row-1",
            aggregateAuthority: "pc-1",
            scope: identity.persistenceScope)
        _ = await outbox.save(
            PersistedOutboxEnvelope(
                scope: identity.persistenceScope,
                aggregateAuthority: "pc-1",
                entries: [entry]),
            revision: outbox.reserveRevision())
        let fence = PersistedHardDeleteFence(
            persistenceScope: identity.persistenceScope,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])
        _ = await store.replaceHardDeleteFence(expected: nil, replacement: fence)

        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        model.triggerStartupHardDeleteRecoveryForTesting()
        await model.awaitStartupHardDeleteRecoveryForTesting()

        let directory = DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent("outbox-row-1.json").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.appendingPathComponent(fence.storageName).appendingPathExtension("json").path))
    }

    func testHardDeleteCleanupWaitsForPersistedRemovalAfterSessionEviction() async {
        let blocker = DrainBlocker()
        let removedProbe = CompletionProbe()
        let store = HardDeleteGatedConversationPersistenceStore(
            owners: ["row-1", "row-0"],
            contentsByConversationId: [
                "row-1": .entries([makePendingOutboxEntry(conversationId: "row-1")]),
                "row-0": .entries([makePendingOutboxEntry(conversationId: "row-0")]),
            ],
            aggregateMembersById: ["pc-1": ["row-1", "row-0"]],
            blocker: blocker,
            removedProbe: removedProbe)
        let model = makeModel(conversationPersistenceStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        model.listStore.upsert(self.conversation(id: "row-1", aggregateId: "pc-1", title: "Root"))
        model.listStore.upsert(self.conversation(id: "row-0", aggregateId: "pc-1", title: "Sibling"))
        let session = model.session(for: "row-1")
        session?.receive(.initSnapshot(.init(
            conversation: self.conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        session?.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await blocker.waitForEntry()
        let waiter = Task { await model.awaitHardDeleteCleanupForTesting(conversationId: "row-1") }

        XCTAssertNil(model.existingSession(for: "row-1"))
        let completedBeforeRelease = await removedProbe.isCompleted()
        XCTAssertFalse(completedBeforeRelease)
        model.pendingOpenConversationId = "row-0"

        await blocker.release()
        await removedProbe.wait()
        await waiter.value
        XCTAssertNil(model.pendingOpenConversationId)

        let lateWaiter = Task { await model.awaitHardDeleteCleanupForTesting(conversationId: "row-1") }
        await lateWaiter.value
    }

    func testAuthoritativeCacheEligibilityRequiresExactConfigurationAndAggregate() async {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-cache-authority-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)

        func writeSnapshot(
            conversationId: String,
            configurationIdentity: APIConfigurationIdentity,
            aggregateAuthority: String
        ) async {
            let snapshot = ConversationSession.PersistedSnapshot(
                conversation: conversation(id: conversationId, aggregateId: "pc-1"),
                messages: [],
                lastSequenceId: 0,
                transcriptGeneration: nil,
                syncedAt: Date(),
                authoritative: .init(
                    configurationIdentity: configurationIdentity,
                    aggregateAuthority: aggregateAuthority,
                    syncedAt: Date()))
            let writer = store.snapshotPersistence(conversationId: conversationId)
            _ = await writer.save(snapshot, revision: writer.reserveRevision())
        }

        await writeSnapshot(
            conversationId: "row-exact",
            configurationIdentity: defaultConfigurationIdentity,
            aggregateAuthority: "pc-1")
        await writeSnapshot(
            conversationId: "row-stale-config",
            configurationIdentity: APIConfigurationIdentity(
                serverURL: "https://stale.example.com",
                credentialGeneration: "stale",
                trustSelfSigned: false),
            aggregateAuthority: "pc-1")
        await writeSnapshot(
            conversationId: "row-wrong-aggregate",
            configurationIdentity: defaultConfigurationIdentity,
            aggregateAuthority: "pc-other")

        XCTAssertTrue(store.hasAuthoritativeCachedSnapshot(
            conversationId: "row-exact",
            configurationIdentity: defaultConfigurationIdentity,
            aggregateAuthority: "pc-1"))
        XCTAssertFalse(store.hasAuthoritativeCachedSnapshot(
            conversationId: "row-stale-config",
            configurationIdentity: defaultConfigurationIdentity,
            aggregateAuthority: "pc-1"))
        XCTAssertFalse(store.hasAuthoritativeCachedSnapshot(
            conversationId: "row-wrong-aggregate",
            configurationIdentity: defaultConfigurationIdentity,
            aggregateAuthority: "pc-1"))
    }


    @MainActor
    func testMisroutedInitRetainsSessionAndAggregateListProjection() async throws {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-1"],
            contentsByConversationId: ["row-1": .entries([])],
            aggregateMembersById: ["pc-old": ["row-1"]])
        let model = AppModel(
            conversationPersistenceStore: store,
            credentialStore: InMemoryCredentialStore())
        let (api, registration) = makeHTTPAPI(probe: SendProbe())
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        model.replaceAPIForTesting(api)
        let oldProjection = conversation(id: "row-1", aggregateId: "pc-old")
        model.listStore.upsert(oldProjection)
        let session = try XCTUnwrap(model.session(for: "row-1", aggregateAuthority: "pc-old"))
        session.receive(.initSnapshot(.init(
            conversation: oldProjection,
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        let listBefore = model.listStore.conversations

        session.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-1", aggregateId: "pc-new"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        XCTAssertTrue(model.existingSession(for: "row-1") === session)
        XCTAssertEqual(model.listStore.conversations, listBefore)
        XCTAssertEqual(model.listStore.aggregateId(forTranscriptRowId: "row-1"), "pc-old")
    }

    func testLoadedDetailResolvesFreshWritableSuccessorToAggregateAuthority() {
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel()
        model.replaceAPIForTesting(api)
        model.productConversationDetailModel(
            for: "pc-1", initialTranscriptRowId: "row-1"
        ).applyForTesting(testProductConversationSnapshot())

        let session = model.session(for: "row-2")

        XCTAssertEqual(session?.aggregateAuthorityIdentity, "pc-1")
    }

    func testFirstSuccessorInitPreservesCanonicalAggregateProjectionMetadata() async throws {
        let baseDirectory = isolatedDiskDirectory()
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory)
        let probe = SendProbe()
        let host = "first-successor-projection.invalid"
        let (api, registration) = makeHTTPAPI(probe: probe, host: host)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        let root = conversation(
            id: "row-1", aggregateId: "pc-1", slug: "canonical-slug", title: "Canonical title")
        var canonical = root
        canonical.task_title = "Canonical task"
        canonical.archived = false
        model.listStore.upsert(canonical)
        let successor = conversation(
            id: "row-2", aggregateId: "pc-1", slug: "successor-slug", title: "Successor title")
        let session = try XCTUnwrap(model.session(for: "row-2", aggregateAuthority: "pc-1"))

        session.receive(.initSnapshot(.init(
            conversation: successor,
            messages: [], agentWorking: true, presentationMode: "working",
            lastSequenceId: 1, pendingAnchorSequenceId: 1,
            pendingEvents: [], pendingTruncated: false)))

        let merged = try XCTUnwrap(model.listStore.conversations.first)
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        await model.listStore.awaitCachePersistence()
        XCTAssertEqual(merged.id, "row-1")
        XCTAssertEqual(merged.slug, "canonical-slug")
        XCTAssertEqual(merged.title, "Canonical title")
        XCTAssertEqual(merged.task_title, "Canonical task")
        XCTAssertEqual(merged.archived, false)
        XCTAssertEqual(merged.state, successor.state)
        XCTAssertEqual(merged.transcriptRowIdentity, "row-1")

        let restored = ConversationListStore(
            hasCachedSnapshot: { $0 == "row-2" },
            context: store.listPersistenceContext!)
        let restoredCanonical = try XCTUnwrap(restored.conversations.first)
        XCTAssertEqual(restoredCanonical.id, "row-1")
        XCTAssertEqual(restoredCanonical.slug, "canonical-slug")
        XCTAssertEqual(restoredCanonical.title, "Canonical title")
        XCTAssertEqual(restoredCanonical.task_title, "Canonical task")
        XCTAssertEqual(restoredCanonical.archived, false)
        XCTAssertEqual(restoredCanonical.state, successor.state)
        XCTAssertEqual(restoredCanonical.transcriptRowIdentity, "row-1")
        XCTAssertEqual(
            restored.cachedNavigationTranscriptRowId(
                forAggregateId: "pc-1",
                latestTranscriptRowId: "row-2"),
            "row-2")
    }

    func testListSuccessorInvalidatesStoppedSingleSegmentCloseCardinality() async throws {
        let probe = SendProbe()
        let host = "list-successor.invalid"
        let successorList = ProductConversationListResponse(product_conversations: [
            .init(
                product_conversation_id: "pc-1",
                canonical_route: "/product-conversations/pc-1",
                canonical_root: .init(transcript_row_id: "row-1", slug: "root", title: "Root"),
                ordinary_lifecycle: .open,
                latest_transcript_row_id: "row-2",
                updated_at: "2025-01-02T04:04:05Z",
                presentation: .state(displayName: "Root", presentationMode: "working"))
        ])
        let registration = TestURLProtocol.install(host: host) { request in
            probe.record(request)
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: nil,
                headerFields: ["Content-Type": "application/json"])!
            return (response, try! JSONEncoder().encode(successorList))
        }
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let model = makeModel()
        model.replaceAPIForTesting(PhoenixAPI(
            baseURL: URL(string: "https://\(host)")!, password: nil,
            allowSelfSigned: false,
            configurationIdentity: .init(
                serverURL: "https://\(host)", credentialGeneration: host,
                trustSelfSigned: false),
            session: URLSession(configuration: {
                let config = URLSessionConfiguration.ephemeral
                config.protocolClasses = [TestURLProtocol.self]
                return config
            }()))!)
        let detail = model.productConversationDetailModel(for: "pc-1")
        detail.applyForTesting(testSingleSegmentProductConversationSnapshot())
        detail.stop()

        await model.refreshList()
        XCTAssertNil(model.listStore.lastError)
        let known = conversation(id: "row-1", aggregateId: "pc-1")

        XCTAssertEqual(
            model.closeUnavailableExplanation(for: known),
            "Open the conversation before closing it.")
        let archived = await model.archive(conversationId: "row-1")
        XCTAssertFalse(archived)
        XCTAssertTrue(probe.archivePostPaths.isEmpty)
    }

    func testCloseAvailabilityRequiresKnownProductConversationCardinality() {
        let model = makeModel()
        let legacy = conversation(id: "legacy", aggregateId: nil)
        XCTAssertEqual(
            model.closeUnavailableExplanation(for: legacy),
            "Close is unavailable until conversation type is confirmed.")

    }

    func testReplacingAPIRevokesExistingSessionsBeforeInstallingNewAuthority() {
        let oldProbe = SendProbe()
        let (oldAPI, oldRegistration) = makeHTTPAPI(probe: oldProbe, host: "replacement-old.invalid")
        defer { TestURLProtocol.uninstall(host: "replacement-old.invalid", owner: oldRegistration) }
        let newProbe = SendProbe()
        let (newAPI, newRegistration) = makeHTTPAPI(probe: newProbe, host: "replacement-new.invalid")
        defer { TestURLProtocol.uninstall(host: "replacement-new.invalid", owner: newRegistration) }
        let model = makeModel()
        model.replaceAPIForTesting(oldAPI)

        XCTAssertNotNil(model.session(for: "row-1"))

        model.replaceAPIForTesting(newAPI)

        XCTAssertNil(model.existingSession(for: "row-1"))
    }

    func testArchiveRejectsStaleAPIAfterPersistedMemberDiscovery() async {
        let store = MutableTestConversationPersistenceStore(contentsByConversationId: [:])
        let discoveryGate = AsyncCandidateGate()
        let oldProbe = SendProbe()
        let (oldAPI, oldRegistration) = makeHTTPAPI(probe: oldProbe, host: "archive-old.invalid")
        defer { TestURLProtocol.uninstall(host: "archive-old.invalid", owner: oldRegistration) }
        let newProbe = SendProbe()
        let (newAPI, newRegistration) = makeHTTPAPI(probe: newProbe, host: "archive-new.invalid")
        defer { TestURLProtocol.uninstall(host: "archive-new.invalid", owner: newRegistration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(oldAPI)
        model.connectivity.setOnlineForTesting(true)
        let conversation = conversation(id: "row-1", aggregateId: "pc-1")
        model.listStore.upsert(conversation)
        model.productConversationDetailModel(
            for: "pc-1", initialTranscriptRowId: "row-1"
        ).applyForTesting(testSingleSegmentProductConversationSnapshot())

        store.persistedMemberDiscoveryGate = discoveryGate

        let archive = Task { @MainActor in await model.archive(conversationId: "row-1") }
        await discoveryGate.waitForEntry()
        model.replaceAPIForTesting(newAPI)
        await discoveryGate.release()

        let archived = await archive.value
        XCTAssertFalse(archived)
        XCTAssertTrue(oldProbe.archivePostPaths.isEmpty)
        XCTAssertTrue(newProbe.archivePostPaths.isEmpty)
        XCTAssertEqual(model.lastActionError, "Conversation settings changed before archiving. Try again.")
    }

    func testArchiveBlocksWhenOutboxOnlyMemberHasVisibleEntries() async {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-latest", "row-outbox"],
            contentsByConversationId: [
                "row-latest": .entries([]),
                "row-outbox": .entries([makePendingOutboxEntry(conversationId: "row-outbox")]),
            ])
        store.persistedMemberDiscoveryOverride = .init(
            currentAuthorityMemberIds: [],
            persistedOutboxOwnerIds: ["row-outbox"])
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        model.connectivity.setOnlineForTesting(true)
        let latest = conversation(id: "row-latest", aggregateId: "pc-1")
        model.listStore.upsert(latest)
        model.productConversationDetailModel(
            for: "pc-1", initialTranscriptRowId: "row-latest"
        ).applyForTesting(testSingleSegmentProductConversationSnapshot())

        let archived = await model.archive(conversationId: "row-latest")

        XCTAssertFalse(archived)
        XCTAssertTrue(probe.archivePostPaths.isEmpty)
        XCTAssertEqual(
            model.lastActionError,
            "This conversation has queued or unconfirmed messages. Retry or discard them before archiving.")
    }

    func testArchiveBlocksWhenPredecessorMemberHasVisibleOutbox() async {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-old", "row-latest"],
            contentsByConversationId: [
                "row-old": .entries([makePendingOutboxEntry(conversationId: "row-old")]),
                "row-latest": .entries([])
            ],
            aggregateMembersById: ["pc-1": ["row-old", "row-latest"]])
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        model.connectivity.setOnlineForTesting(true)
        model.listStore.upsert(conversation(id: "row-latest", aggregateId: "pc-1"))

        let archived = await model.archive(conversationId: "row-latest")

        XCTAssertFalse(archived)
        XCTAssertTrue(probe.archivePostPaths.isEmpty)
    }

    func testArchiveRechecksCardinalityAfterPersistedMemberDiscovery() async {
        let store = MutableTestConversationPersistenceStore(contentsByConversationId: [:])
        let discoveryGate = AsyncCandidateGate()
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        model.connectivity.setOnlineForTesting(true)
        model.listStore.upsert(conversation(id: "row-1", aggregateId: "pc-1"))
        let detail = model.productConversationDetailModel(
            for: "pc-1", initialTranscriptRowId: "row-1")
        detail.applyForTesting(testSingleSegmentProductConversationSnapshot())
        store.persistedMemberDiscoveryGate = discoveryGate

        let archive = Task { @MainActor in await model.archive(conversationId: "row-1") }
        await discoveryGate.waitForEntry()
        detail.applyForTesting(testProductConversationSnapshot())
        await discoveryGate.release()

        let archived = await archive.value
        XCTAssertTrue(archived)
        XCTAssertEqual(probe.archivePostPaths, ["/api/chains/row-1/archive"])
    }

    func testArchiveContinuedAggregateUsesCanonicalChainEndpoint() async {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-root", "row-successor"],
            contentsByConversationId: [
                "row-root": .entries([]),
                "row-successor": .entries([])
            ],
            aggregateMembersById: ["pc-1": ["row-root", "row-successor"]])
        let probe = SendProbe()
        let host = "canonical-root-archive.invalid"
        let (api, registration) = makeHTTPAPI(probe: probe, host: host)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let model = AppModel(
            conversationPersistenceStore: store,
            credentialStore: InMemoryCredentialStore())
        model.replaceAPIForTesting(api)
        model.connectivity.setOnlineForTesting(true)
        model.listStore.upsert(conversation(id: "row-successor", aggregateId: "pc-1"))
        model.productConversationDetailModel(
            for: "pc-1",
            initialTranscriptRowId: "row-successor"
        ).applyForTesting(testProductConversationSnapshot())

        let continued = try! XCTUnwrap(model.listStore.conversations.first)
        XCTAssertNil(model.closeUnavailableExplanation(for: continued))

        let archived = await model.archive(conversationId: "row-successor")

        XCTAssertTrue(archived)
        XCTAssertEqual(probe.archivePostPaths, ["/api/chains/row-1/archive"])
        XCTAssertTrue(model.listStore.conversations.isEmpty)
    }

    func testArchiveProceedsForSingleSegmentProductConversation() async {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-archive-tests-\(UUID().uuidString)")
        let store = DiskConversationPersistenceStore(
            baseDirectory: baseDirectory,
            context: DiskStore.versionedContext(baseDirectory: baseDirectory))
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        model.connectivity.setOnlineForTesting(true)
        let knownSingleSegment = conversation(id: "row-1", aggregateId: "pc-1")
        model.listStore.upsert(knownSingleSegment)
        model.productConversationDetailModel(
            for: "pc-1",
            initialTranscriptRowId: "row-1"
        ).applyForTesting(testSingleSegmentProductConversationSnapshot())
        XCTAssertNil(model.closeUnavailableExplanation(for: knownSingleSegment))

        let archived = await model.archive(conversationId: "row-1")

        XCTAssertTrue(archived)
        XCTAssertEqual(probe.archivePostPaths, ["/api/conversations/row-1/archive"])
        XCTAssertTrue(model.listStore.conversations.isEmpty)

        let restored = ConversationListStore(
            hasCachedSnapshot: { _ in false },
            context: store.listPersistenceContext!)
        XCTAssertTrue(restored.conversations.isEmpty)
    }

    func testAggregateNotFoundCleanupOnlyRemovesExactScopedPersistedMembers() async throws {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-aggregate-cleanup-scope-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)
        let exactId = "row-exact"
        let foreignId = "row-foreign"
        let unscopedId = "row-unscoped"

        func writeSnapshot(
            conversationId: String,
            authority: ConversationSession.PersistedSnapshotAuthority?
        ) async {
            let snapshot = ConversationSession.PersistedSnapshot(
                conversation: conversation(id: conversationId, aggregateId: "pc-1"),
                messages: [],
                lastSequenceId: 0,
                transcriptGeneration: nil,
                syncedAt: Date(),
                authoritative: authority)
            let writer = store.snapshotPersistence(conversationId: conversationId)
            _ = await writer.save(snapshot, revision: writer.reserveRevision())
        }

        await writeSnapshot(
            conversationId: exactId,
            authority: .init(
                configurationIdentity: defaultConfigurationIdentity,
                aggregateAuthority: "pc-1",
                syncedAt: Date()))
        await writeSnapshot(
            conversationId: foreignId,
            authority: .init(
                configurationIdentity: APIConfigurationIdentity(
                    serverURL: "https://foreign.example.com",
                    credentialGeneration: "foreign",
                    trustSelfSigned: false),
                aggregateAuthority: "pc-1",
                syncedAt: Date()))
        await writeSnapshot(conversationId: unscopedId, authority: nil)

        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(PhoenixAPI(
            baseURL: URL(string: defaultConfigurationIdentity.serverURL)!,
            password: nil,
            allowSelfSigned: defaultConfigurationIdentity.trustSelfSigned,
            configurationIdentity: defaultConfigurationIdentity)!)
        await model.forceAggregateNotFoundCleanupForTesting(
            aggregateId: "pc-1",
            transcriptRowId: foreignId,
            memberIds: [exactId, foreignId, unscopedId])

        let directory = DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
        XCTAssertFalse(FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("conv-\(exactId).json").path))
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("conv-\(foreignId).json").path))
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("conv-\(unscopedId).json").path))
    }

    func testAggregateNotFoundCleanupIncludesPersistedUnloadedPredecessor() async {
        let inactiveSegmentId = "row-inactive"
        let store = MutableTestConversationPersistenceStore(
            owners: [inactiveSegmentId],
            contentsByConversationId: [inactiveSegmentId: .entries([makePendingOutboxEntry(conversationId: inactiveSegmentId)])],
            snapshotsByConversationId: [inactiveSegmentId],
            aggregateMembersById: ["pc-1": [inactiveSegmentId]])
        let model = makeModel(conversationPersistenceStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let detail = model.productConversationDetailModel(for: "pc-1")
        detail.applyForTesting(testProductConversationSnapshot())

        await model.forceAggregateNotFoundCleanupForTesting(
            aggregateId: "pc-1",
            transcriptRowId: "row-2",
            memberIds: ["row-1", "row-2"])
        _ = await model.awaitCurrentPersistedOutboxDrainForTesting()

        XCTAssertFalse(store.snapshotsByConversationId.contains(inactiveSegmentId))
        if case .missing = store.inspectOutbox(conversationId: inactiveSegmentId).state {
        } else {
            XCTFail("expected inactive persisted outbox to be removed")
        }
        XCTAssertFalse(store.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains(inactiveSegmentId))
    }

    @MainActor
    func testAggregateNotFoundCleanupClearsInactivePersistedSegmentState() async {
        let inactiveSegmentId = "row-inactive"
        let store = MutableTestConversationPersistenceStore(
            owners: [inactiveSegmentId],
            contentsByConversationId: [inactiveSegmentId: .entries([makePendingOutboxEntry(conversationId: inactiveSegmentId)])],
            snapshotsByConversationId: [inactiveSegmentId])
        let model = makeModel(conversationPersistenceStore: store)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let detail = model.productConversationDetailModel(for: "pc-1")
        detail.applyForTesting(testProductConversationSnapshot())

        await model.forceAggregateNotFoundCleanupForTesting(
            aggregateId: "pc-1",
            transcriptRowId: "row-2",
            memberIds: ["row-1", "row-2", inactiveSegmentId])
        _ = await model.awaitCurrentPersistedOutboxDrainForTesting()

        XCTAssertFalse(store.snapshotsByConversationId.contains(inactiveSegmentId))
        if case .missing = store.inspectOutbox(conversationId: inactiveSegmentId).state {
        } else {
            XCTFail("expected inactive persisted outbox to be removed")
        }
        XCTAssertFalse(store.persistedOutboxOwnersSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains(inactiveSegmentId))
        XCTAssertNil(model.existingSession(for: inactiveSegmentId))
    }

    @MainActor
    func testStaleInvalidationCannotEvictReplacementDetail() async {
        let model = makeModel()
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)

        let old = model.productConversationDetailModel(for: "pc-1")
        old.invalidateConfiguration()
        let fresh = model.productConversationDetailModel(for: "pc-1")
        XCTAssertFalse(old === fresh)

        old.invalidateConfiguration()

        XCTAssertTrue(model.productConversationDetailModel(for: "pc-1") === fresh)
    }

    func testProductConversationDetailModelPrimesInitialTranscriptRowId() {
        let model = makeModel()
        let detail = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: PhoenixAPI(baseURL: URL(string: "https://example.com")!, password: nil, allowSelfSigned: true, configurationIdentity: APIConfigurationIdentity(serverURL: "https://example.com", credentialGeneration: "test-detail", trustSelfSigned: true))!,
            connectivity: model.connectivity,
            sessionProvider: { _, _ in nil })

        detail.primeInitialTranscriptRowId("row-1")

        XCTAssertEqual(detail.initialTranscriptRowId, "row-1")
    }
}
