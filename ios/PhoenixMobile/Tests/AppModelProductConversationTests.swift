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

    func loadRecord(account: String) -> AppModel.CredentialRecord? { record }
    func saveRecord(_ record: AppModel.CredentialRecord, account: String) throws {
        if failNextSave {
            failNextSave = false
            throw Fault.saveRecordFailed
        }
        self.record = record
    }
    func deleteRecord(account: String) {
        record = nil
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
    func persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity) -> Set<String> { outboxStore.ownerTranscriptRowIds }
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

    func pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity) async -> Set<String> {
        return Set(outboxStore.ownerTranscriptRowIds.filter { conversationId in
            outboxStore.inspect(conversationId: conversationId).hasPendingSendableEntries
        })
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
    func persistHardDeleteFence(_ fence: PersistedHardDeleteFence) async -> Bool { true }
    func hardDeleteFences(configurationIdentity: APIConfigurationIdentity) -> HardDeleteFenceLoadResult { .accessible([]) }
    func retireHardDeleteFence(_ fence: PersistedHardDeleteFence) async {}
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

    func pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity) async -> Set<String> {
        await wrapped.pendingOutboxOwnerTranscriptRowIds(scope: scope)
    }

    func persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity) -> Set<String> {
        wrapped.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: scope)
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
    func persistHardDeleteFence(_ fence: PersistedHardDeleteFence) async -> Bool { true }
    func hardDeleteFences(configurationIdentity: APIConfigurationIdentity) -> HardDeleteFenceLoadResult { .accessible([]) }
    func retireHardDeleteFence(_ fence: PersistedHardDeleteFence) async {}
    func removeAllPersistedConversationState() async {
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

    func pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity) async -> Set<String> { outboxStore.ownerTranscriptRowIds }
    func persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity) -> Set<String> { outboxStore.ownerTranscriptRowIds }
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
    func persistHardDeleteFence(_ fence: PersistedHardDeleteFence) async -> Bool { true }
    func hardDeleteFences(configurationIdentity: APIConfigurationIdentity) -> HardDeleteFenceLoadResult { .accessible([]) }
    func retireHardDeleteFence(_ fence: PersistedHardDeleteFence) async {}
    func removeAllPersistedConversationState() async {}
}

@MainActor
final class GatedConversationPersistenceStore: ConversationPersistenceStore {
    var listPersistenceContext: VersionedDiskContext? { nil }
    var persistenceScope: PersistenceScopeIdentity? { nil }
    func persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity) -> Set<String> { outboxStore.ownerTranscriptRowIds }
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

    func pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity) async -> Set<String> {
        await gate.markEntered()
        await gate.awaitRelease()
        return Set(outboxStore.ownerTranscriptRowIds.filter { conversationId in
            outboxStore.inspect(conversationId: conversationId).hasPendingSendableEntries
        })
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
    func persistHardDeleteFence(_ fence: PersistedHardDeleteFence) async -> Bool { true }
    func hardDeleteFences(configurationIdentity: APIConfigurationIdentity) -> HardDeleteFenceLoadResult { .accessible([]) }
    func retireHardDeleteFence(_ fence: PersistedHardDeleteFence) async {}
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
    func persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity) -> Set<String> { outboxStore.ownerTranscriptRowIds }
    var snapshotsByConversationId: Set<String>
    var aggregateMembersById: [String: Set<String>]
    var onPendingOutboxOwnerTranscriptRowIds: (() async -> Set<String>)?
    var hardDeleteFenceLoadResult: HardDeleteFenceLoadResult = .accessible([])
    private(set) var hardDeleteFenceLoadCount = 0
    private(set) var pendingOutboxDiscoveryCount = 0
    private let outboxStore: InMemoryOutboxStore

    init(owners: Set<String> = [], contentsByConversationId: [String: PersistedOutboxStoreContents], snapshotsByConversationId: Set<String> = [], aggregateMembersById: [String: Set<String>] = [:]) {
        self.snapshotsByConversationId = snapshotsByConversationId
        self.aggregateMembersById = aggregateMembersById
        self.outboxStore = InMemoryOutboxStore(contentsByConversationId: contentsByConversationId, owners: owners)
    }

    func pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity) async -> Set<String> {
        pendingOutboxDiscoveryCount += 1
        if let onPendingOutboxOwnerTranscriptRowIds {
            return await onPendingOutboxOwnerTranscriptRowIds()
        }
        return Set(outboxStore.ownerTranscriptRowIds.filter { conversationId in
            outboxStore.inspect(conversationId: conversationId).hasPendingSendableEntries
        })
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
    func persistHardDeleteFence(_ fence: PersistedHardDeleteFence) async -> Bool { true }
    func hardDeleteFences(configurationIdentity: APIConfigurationIdentity) -> HardDeleteFenceLoadResult {
        hardDeleteFenceLoadCount += 1
        return hardDeleteFenceLoadResult
    }
    func retireHardDeleteFence(_ fence: PersistedHardDeleteFence) async {}
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
final class InMemoryCoordinatorIdentityStore: CoordinatorIdentityStore {
    var receiptsByConfigurationIdentity: [APIConfigurationIdentity: CoordinatorIdentityReceipt]

    init(_ value: String? = nil, configurationIdentity: APIConfigurationIdentity = APIConfigurationIdentity(serverURL: "https://example.com", credentialGeneration: "test-default", trustSelfSigned: true)) {
        if let value {
            receiptsByConfigurationIdentity = [configurationIdentity: CoordinatorIdentityReceipt(configurationIdentity: configurationIdentity, conversationId: value)]
        } else {
            receiptsByConfigurationIdentity = [:]
        }
    }

    var value: String? {
        receiptsByConfigurationIdentity.values.first?.conversationId
    }

    func load(configurationIdentity: APIConfigurationIdentity) -> CoordinatorIdentityReceipt? {
        receiptsByConfigurationIdentity[configurationIdentity]
    }

    func save(_ receipt: CoordinatorIdentityReceipt) {
        receiptsByConfigurationIdentity[receipt.configurationIdentity] = receipt
    }

    func clear(configurationIdentity: APIConfigurationIdentity) {
        receiptsByConfigurationIdentity.removeValue(forKey: configurationIdentity)
    }

    func clearAll() {
        receiptsByConfigurationIdentity.removeAll()
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

    private func makeHTTPAPI(
        probe: SendProbe,
        host: String = "phoenix.invalid",
        productConversationStatusCode: Int = 200,
        productConversationBody: Data = Data("{}".utf8),
        chatStatusCode: Int = 200,
        chatBody: Data = Data("{\"queued\":false}".utf8)
    ) -> (api: PhoenixAPI, registration: UUID) {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TestURLProtocol.self]
        let registration = TestURLProtocol.install(host: host) { (request: URLRequest) in
            probe.record(request)
            let url = request.url!
            if request.httpMethod == "GET", url.path.contains("/api/product-conversations/") {
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
            configurationIdentity: APIConfigurationIdentity(serverURL: "https://\(host)", credentialGeneration: host, trustSelfSigned: false),
            session: session,
            streamSession: session)!
        return (api, registration)
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
        XCTAssertTrue(store.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
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

        store.hardDeleteFenceLoadResult = .accessible([])
        model.connectivity.setOnlineForTesting(true)
        await gate.waitForEntry()
        guard let generation = model.currentPersistedOutboxDrainGenerationForTesting() else {
            XCTFail("expected connectivity recovery to schedule a persisted outbox drain")
            return
        }
        let session = model.existingSession(for: "row-1") ?? model.session(for: "row-1")
        session?.receive(.initSnapshot(.init(
            conversation: conversation(id: "row-1", aggregateId: "pc-1"),
            messages: [],
            agentWorking: false,
            presentationMode: "idle",
            lastSequenceId: 0,
            pendingAnchorSequenceId: 0,
            pendingEvents: [],
            pendingTruncated: false)))
        _ = await session?.flushSnapshotPersistence()
        await gate.release()
        let drain = await model.awaitPersistedOutboxDrainForTesting(generation: generation)

        XCTAssertEqual(drain, .completed(generation))
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
        XCTAssertFalse(store.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
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
        let pendingAIds = await storeA.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        let pendingBIds = await storeB.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(pendingAIds, [])
        XCTAssertEqual(pendingBIds, ["row-b"])
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

        let pendingAIds = await storeA.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        let pendingBIds = await storeB.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(pendingAIds, [])
        XCTAssertEqual(pendingBIds, ["row-b"])
        XCTAssertEqual(storeA.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")), [])
        XCTAssertEqual(storeB.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")), ["row-b"])
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

        XCTAssertNil(identityStore.receiptsByConfigurationIdentity[api.configurationIdentity])
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

        XCTAssertTrue(firstStore.receiptsByConfigurationIdentity.isEmpty)
        XCTAssertEqual(secondStore.receiptsByConfigurationIdentity[secondIdentity]?.conversationId, "coordinator-b")
        XCTAssertEqual(second.coordinatorConversationId, "coordinator-b")
    }

    func testRebuildAPIKeepsCurrentConfigurationCoordinatorReceipt() {
        let identityStore = InMemoryCoordinatorIdentityStore()
        let model = makeModel(coordinatorIdentityStore: identityStore)
        model.configureForTesting(serverURL: "https://example.com", trustSelfSigned: true)
        let identity = model.configurationIdentity!
        identityStore.receiptsByConfigurationIdentity[identity] = CoordinatorIdentityReceipt(configurationIdentity: identity, conversationId: "coordinator-row")
        model.replaceAPIForTesting(PhoenixAPI(baseURL: URL(string: identity.serverURL)!, password: nil, allowSelfSigned: identity.trustSelfSigned, configurationIdentity: identity)!)

        XCTAssertEqual(model.coordinatorConversationId, "coordinator-row")
        XCTAssertEqual(identityStore.receiptsByConfigurationIdentity[identity]?.conversationId, "coordinator-row")
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

        XCTAssertEqual(store.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")), ["row-1"])
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

        let ids = await storeA.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))

        XCTAssertEqual(ids, ["row-1"])
        let storeBPendingIds = await storeB.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(storeBPendingIds, ["row-external"])
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

        let storeBPendingIdsAfterHandleARemoval = await storeB.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(storeBPendingIdsAfterHandleARemoval, ["row-external", "row-1"])
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

        let storeAPendingIds = await storeA.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(storeAPendingIds, [])
        let storeBPendingIds = await storeB.pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default"))
        XCTAssertEqual(storeBPendingIds, ["row-b"])
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
        XCTAssertFalse(store.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
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
        XCTAssertFalse(store.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-1"))
        XCTAssertFalse(store.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains("row-0"))
        XCTAssertNil(model.existingSession(for: "row-1"))
        XCTAssertNil(model.existingSession(for: "row-0"))
        let replacement = model.productConversationDetailModel(for: "pc-1")
        XCTAssertFalse(replacement === detail)
    }

    @MainActor
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

        await model.forceAggregateNotFoundCleanupForTesting(
            aggregateId: "pc-1",
            transcriptRowId: "row-1",
            memberIds: ["row-1"])

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

        XCTAssertTrue(store.hardDeleteFences(configurationIdentity: APIConfigurationIdentity(
                serverURL: "https://example.com",
                credentialGeneration: "test-default",
                trustSelfSigned: true)) == .accessible([]))
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
            configurationIdentity: identityA,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])
        let fenceB = PersistedHardDeleteFence(
            configurationIdentity: identityB,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])

        XCTAssertNotEqual(fenceA.storageName, fenceB.storageName)
        let savedA = await store.persistHardDeleteFence(fenceA)
        let savedB = await store.persistHardDeleteFence(fenceB)
        XCTAssertTrue(savedA)
        XCTAssertTrue(savedB)
        await store.retireHardDeleteFence(fenceA)

        XCTAssertTrue(store.hardDeleteFences(configurationIdentity: identityA) == .accessible([]))
        XCTAssertEqual(store.hardDeleteFences(configurationIdentity: identityB), .accessible([fenceB]))
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
            configurationIdentity: identity,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])
        _ = await store.persistHardDeleteFence(fence)

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
            configurationIdentity: identity,
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])
        _ = await store.persistHardDeleteFence(fence)

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
        let waiter = Task { await model.awaitHardDeleteCleanupForTesting(conversationId: "row-1") }

        session?.receive(.conversationHardDeleted(seq: 1, conversationId: "row-1"))
        await blocker.waitForEntry()

        XCTAssertNil(model.existingSession(for: "row-1"))
        let completedBeforeRelease = await removedProbe.isCompleted()
        XCTAssertFalse(completedBeforeRelease)

        await blocker.release()
        await removedProbe.wait()
        await waiter.value

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

    func testArchiveRoutesContinuedAggregateThroughAuthoritativeCanonicalRoot() async {
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

        let archived = await model.archive(conversationId: "row-successor")

        XCTAssertTrue(archived)
        XCTAssertEqual(probe.archivePostPaths, ["/api/conversations/row-1/archive"])
    }

    func testArchiveProceedsWhenEveryAggregateMemberOutboxIsEmpty() async {
        let store = MutableTestConversationPersistenceStore(
            owners: ["row-old", "row-latest"],
            contentsByConversationId: ["row-old": .entries([]), "row-latest": .entries([])],
            aggregateMembersById: ["pc-1": ["row-old", "row-latest"]])
        let probe = SendProbe()
        let (api, registration) = makeHTTPAPI(probe: probe)
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let model = makeModel(conversationPersistenceStore: store)
        model.replaceAPIForTesting(api)
        model.connectivity.setOnlineForTesting(true)
        model.listStore.upsert(conversation(id: "row-latest", aggregateId: "pc-1"))

        let archived = await model.archive(conversationId: "row-latest")

        XCTAssertTrue(archived)
        XCTAssertEqual(probe.archivePostPaths.count, 1)
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
        XCTAssertFalse(store.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains(inactiveSegmentId))
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
        XCTAssertFalse(store.persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity(serverURL: "https://example.com", credentialGeneration: "test-default")).contains(inactiveSegmentId))
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
            sessionProvider: { _ in nil })

        detail.primeInitialTranscriptRowId("row-1")

        XCTAssertEqual(detail.initialTranscriptRowId, "row-1")
    }
}
