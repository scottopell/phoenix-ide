import Foundation
import Observation

enum PersistedOutboxStoreContents: Equatable {
    case missing
    case entries([OutboxEntry])
    case inaccessible
}

struct OutboxStoreInspection: Equatable {
    enum FileState: Equatable {
        case missing
        case accessible(scope: PersistenceScopeIdentity?, aggregateAuthority: String?, entries: [OutboxEntry])
        case inaccessible
        case incompatibleNewerVersion
    }

    let conversationId: String
    let state: FileState

    var visibleEntries: [OutboxEntry] {
        guard case .accessible(_, _, let entries) = state else { return [] }
        return entries.filter { $0.conversationId == conversationId && $0.isVisible }
    }

    var hasPendingSendableEntries: Bool {
        visibleEntries.contains { $0.status == .pending && !$0.acceptedByServer }
    }

    var aggregateAuthority: String? {
        guard case .accessible(_, let aggregateAuthority, _) = state else { return nil }
        return aggregateAuthority
    }

    var scope: PersistenceScopeIdentity? {
        guard case .accessible(let scope, _, _) = state else { return nil }
        return scope
    }
}

@MainActor
final class OutboxPersistenceHandle {
    private let inspectImpl: (String) -> OutboxStoreInspection
    private let reserveImpl: () -> Int
    private let saveImpl: (PersistedOutboxEnvelope, Int) async -> Bool
    private let removeImpl: (Int) async -> Void

    init(
        inspect: @escaping (String) -> OutboxStoreInspection,
        reserveRevision: @escaping () -> Int,
        save: @escaping (PersistedOutboxEnvelope, Int) async -> Bool,
        remove: @escaping (Int) async -> Void
    ) {
        self.inspectImpl = inspect
        self.reserveImpl = reserveRevision
        self.saveImpl = save
        self.removeImpl = remove
    }

    func inspect(conversationId: String) -> OutboxStoreInspection {
        inspectImpl(conversationId)
    }

    func loadVisibleEntries(conversationId: String) -> PersistedOutboxStoreContents {
        let inspection = inspect(conversationId: conversationId)
        switch inspection.state {
        case .missing:
            return .missing
        case .accessible:
            return .entries(inspection.visibleEntries)
        case .inaccessible, .incompatibleNewerVersion:
            return .inaccessible
        }
    }

    func reserveRevision() -> Int { reserveImpl() }
    func save(_ envelope: PersistedOutboxEnvelope, revision: Int) async -> Bool { await saveImpl(envelope, revision) }
    func remove(revision: Int) async { await removeImpl(revision) }

    static func disk(
        conversationId: String,
        baseDirectory: URL? = nil,
        context: VersionedDiskContext? = nil,
        aggregateAuthority: String? = nil,
        scope: PersistenceScopeIdentity? = nil
    ) -> OutboxPersistenceHandle {
        let resolvedBaseDirectory = baseDirectory ?? DiskStore.baseDirectory
        let directory = DiskStore.phoenixMobileDirectory(baseDirectory: resolvedBaseDirectory)
        let source = directory.appendingPathComponent("outbox-\(conversationId)").appendingPathExtension("json")
        let writer = (context ?? DiskStore.versionedContext(baseDirectory: resolvedBaseDirectory)).writer(
            destinationURL: source,
            version: Outbox.schemaVersion)
        return OutboxPersistenceHandle(
            inspect: { requestedConversationId in
                switch DiskStore.loadVersionedResult(
                    PersistedOutboxEnvelope.self,
                    source: source,
                    version: Outbox.schemaVersion,
                    migrate: { storedVersion, fileData in
                        guard let entries = migrateLegacyOutboxEnvelope(
                            storedVersion: storedVersion,
                            fileData: fileData)
                        else { return nil }
                        return PersistedOutboxEnvelope(
                            scope: scope,
                            aggregateAuthority: aggregateAuthority,
                            entries: entries)
                    })
                {
                case .missing:
                    return OutboxStoreInspection(conversationId: requestedConversationId, state: .missing)
                case .value(let envelope):
                    return OutboxStoreInspection(
                        conversationId: requestedConversationId,
                        state: .accessible(
                            scope: envelope.scope,
                            aggregateAuthority: envelope.aggregateAuthority,
                            entries: envelope.entries))
                case .incompatible:
                    return OutboxStoreInspection(conversationId: requestedConversationId, state: .incompatibleNewerVersion)
                case .unreadable:
                    return OutboxStoreInspection(conversationId: requestedConversationId, state: .inaccessible)
                }
            },
            reserveRevision: { writer.reserveRevision() },
            save: { envelope, revision in await writer.save(envelope, revision: revision) },
            remove: { revision in await writer.remove(revision: revision) })
    }
}

private struct LegacyPersistedOutboxEnvelope: Decodable {
    let payload: [OutboxEntry]
}

func migrateLegacyOutboxEnvelope(storedVersion: Int, fileData: Data) -> [OutboxEntry]? {
    guard storedVersion == 1 else { return nil }
    return try? JSONDecoder().decode(LegacyPersistedOutboxEnvelope.self, from: fileData).payload
}

struct PersistedOutboxEnvelope: Codable, Equatable, Sendable {
    let scope: PersistenceScopeIdentity?
    let aggregateAuthority: String?
    let entries: [OutboxEntry]
}

/// A locally-authored message that has not yet been confirmed by
/// authoritative server history. Implements the client-side delivery
/// contract in specs/user_message_queue/user_message_queue.allium:
/// `localId` doubles as the POST `message_id`, so retries are idempotent
/// and reconciliation joins that submitted identity to server history.
struct PersistenceScopeIdentity: Codable, Equatable, Hashable, Sendable {
    let serverEndpoint: String
    let credentialGeneration: String

    init(serverURL: String, credentialGeneration: String) {
        self.serverEndpoint = Self.normalizedServerEndpoint(serverURL)
        self.credentialGeneration = credentialGeneration
    }

    private static func normalizedServerEndpoint(_ serverURL: String) -> String {
        guard let url = URL(string: serverURL), let scheme = url.scheme, let host = url.host else {
            return serverURL.lowercased()
        }
        var normalized = "\(scheme.lowercased())://\(host.lowercased())"
        if let port = url.port { normalized += ":\(port)" }
        return normalized
    }
}

struct OutboxEntry: Codable, Identifiable, Equatable, Sendable {
    enum Status: String, Codable, Sendable {
        /// Authored; awaiting send or awaiting reflection in server history.
        case pending
        /// The server definitively rejected the send; manual retry required.
        case failed
        /// Accepted onto the steering queue (conversation was busy).
        case steeringQueued
        /// The server accepted the POST but the message hasn't shown up in
        /// history within the expected window — surfaced with a retry
        /// affordance instead of an indefinite spinner.
        case recoverableInconsistency
        /// Terminal: observed in authoritative history.
        case reconciled
        /// Terminal: user discarded the entry.
        case dismissed
    }

    var localId: String
    var conversationId: String
    var text: String
    var images: [ImagePayload]
    var status: Status
    var acceptedByServer: Bool
    var createdAt: Date
    /// When the server accepted the POST. The staleness window for the
    /// recoverable-inconsistency surface runs from here, not createdAt — a
    /// message composed offline an hour ago and accepted just now deserves
    /// the full window before being flagged. Optional: pre-acceptance
    /// entries (and rows persisted before this field existed) have none;
    /// absent-with-accepted falls back to createdAt.
    var acceptedAt: Date?
    var lastError: String?
    var attemptCount: Int

    var id: String { localId }

    var isVisible: Bool {
        status != .reconciled && status != .dismissed
    }

    func isReflected(in authoritativeMessageIds: Set<String>) -> Bool {
        authoritativeMessageIds.contains(localId)
            || authoritativeMessageIds.contains("\(conversationId):\(localId)")
    }
}

/// Per-conversation persistent outbox. Entries survive app restarts and
/// render immediately on conversation open, so a message queued in a tunnel
/// is never lost — it sends when connectivity returns.
@MainActor
@Observable
final class Outbox {
    enum StoredContents: Equatable {
        case empty
        case hasVisibleEntries
        case inaccessible
    }

    let conversationId: String
    private(set) var entries: [OutboxEntry] = []
    private var suppressedMessageIds: Set<String> = []
    /// False when the last disk write failed (storage full/unavailable).
    /// Queued entries then exist in memory only — the UI warns that they
    /// won't survive an app restart. Cleared by the next successful write.
    private(set) var persistenceHealthy = true
    private var storageWritable = true
    private let persistence: OutboxPersistenceHandle
    private var latestPersistenceRevision = 0

    /// v1 stored visible OutboxEntry values directly. v2 stores a scoped envelope.
    static let schemaVersion = 2

    let aggregateAuthority: String?
    let persistenceScope: PersistenceScopeIdentity?

    init(
        conversationId: String,
        aggregateAuthority: String?,
        persistenceScope: PersistenceScopeIdentity?,
        persistence: OutboxPersistenceHandle
    ) {
        self.conversationId = conversationId
        self.aggregateAuthority = aggregateAuthority
        self.persistenceScope = persistenceScope
        self.persistence = persistence
        // Rehydrate only entries tagged with this conversation — a foreign
        // entry can never reconcile here and must not render (spec rule
        // RehydrateQueueForConversationOnly).
        switch persistence.loadVisibleEntries(conversationId: conversationId) {
        case .missing:
            entries = []
        case .entries(let loaded):
            entries = loaded.filter { $0.conversationId == conversationId && $0.isVisible }
        case .inaccessible:
            entries = []
            persistenceHealthy = false
            storageWritable = false
        }
    }

    var visibleEntries: [OutboxEntry] {
        entries.filter { $0.isVisible && !$0.isReflected(in: suppressedMessageIds) }
    }

    var hasSendableEntries: Bool {
        entries.contains { $0.status == .pending && !$0.acceptedByServer }
    }

    @discardableResult
    private func persist() async -> Bool {
        guard storageWritable else {
            persistenceHealthy = false
            return false
        }
        let revision = persistence.reserveRevision()
        latestPersistenceRevision = revision
        let snapshot = PersistedOutboxEnvelope(
            scope: persistenceScope,
            aggregateAuthority: aggregateAuthority,
            entries: entries.filter(\.isVisible))
        let saved = await persistence.save(snapshot, revision: revision)
        if latestPersistenceRevision == revision {
            persistenceHealthy = saved
        }
        return saved
    }

    private func persistEventually() {
        guard storageWritable else {
            persistenceHealthy = false
            return
        }
        let revision = persistence.reserveRevision()
        latestPersistenceRevision = revision
        let snapshot = PersistedOutboxEnvelope(
            scope: persistenceScope,
            aggregateAuthority: aggregateAuthority,
            entries: entries.filter(\.isVisible))
        Task {
            let saved = await persistence.save(snapshot, revision: revision)
            guard latestPersistenceRevision == revision else { return }
            persistenceHealthy = saved
        }
    }

    /// Re-establish the enqueue-before-POST durability point immediately
    /// before delivery. A transiently failed enqueue write can recover here;
    /// a continuing failure keeps every entry unsendable.
    func prepareForDelivery() async -> Bool {
        await persist()
    }

    func flushPersistence() async -> Bool {
        await persist()
    }

    private func invalidateForRemoval() -> Int {
        entries.removeAll()
        suppressedMessageIds.removeAll()
        persistenceHealthy = true
        storageWritable = false
        let revision = persistence.reserveRevision()
        latestPersistenceRevision = revision
        return revision
    }

    /// A hard-deleted conversation owns no remaining local delivery state.
    func clear() {
        let revision = invalidateForRemoval()
        Task { await persistence.remove(revision: revision) }
    }

    /// Invalidate every queued write and wait until the revision fence has
    /// removed this store. Later-arriving older writes are rejected.
    func clearAndWait() async {
        let revision = invalidateForRemoval()
        await persistence.remove(revision: revision)
    }

    private func update(_ localId: String, _ mutate: (inout OutboxEntry) -> Void) {
        guard let idx = entries.firstIndex(where: { $0.localId == localId }) else { return }
        mutate(&entries[idx])
        persistEventually()
    }

    // MARK: - Contract transitions

    /// EnqueueLocalMessage: the entry exists (and persists) before any POST
    /// is attempted, so navigation or connection loss cannot erase the
    /// user's words.
    func enqueue(text: String, images: [ImagePayload] = []) async -> OutboxEntry? {
        let entry = OutboxEntry(
            localId: UUID().uuidString.lowercased(),
            conversationId: conversationId,
            text: text,
            images: images,
            status: .pending,
            acceptedByServer: false,
            createdAt: Date(),
            acceptedAt: nil,
            lastError: nil,
            attemptCount: 0)
        entries.append(entry)
        _ = await persist()
        return entry
    }

    func markAttempted(_ localId: String) {
        update(localId) { $0.attemptCount += 1 }
    }

    /// PostAcceptedAsSteeringQueued / PostAcceptedAsPendingReflection.
    /// Terminal entries are immune: a replayed steer_message_queued event
    /// (SSE pending-events ring) or a POST completing after dismissal must
    /// not resurrect an entry the user discarded or that already reconciled.
    func markAccepted(_ localId: String, steering: Bool) {
        update(localId) { entry in
            guard entry.isVisible else { return }
            entry.acceptedByServer = true
            entry.acceptedAt = Date()
            entry.lastError = nil
            if steering {
                entry.status = .steeringQueued
            }
            // Non-steering accept stays `pending` until authoritative
            // history reflects it (reconcile below).
        }
    }

    /// PostFailedIsRetryable — for definitive server rejections. Transport
    /// failures (offline, timeouts) do NOT call this: those entries stay
    /// `pending` and are auto-retried when connectivity returns, which is
    /// safe because message_id makes resends idempotent.
    func markFailed(_ localId: String, error: String) {
        update(localId) { entry in
            guard entry.isVisible else { return }
            entry.status = .failed
            entry.lastError = error
        }
    }

    /// RetryFailedMessage. Clears `acceptedByServer` so the drain loop
    /// (which skips already-accepted entries) actually re-POSTs — safe by
    /// message_id idempotency, and required for the recoverable-
    /// inconsistency path where the previous accept evidently went nowhere.
    func retry(_ localId: String) {
        update(localId) { entry in
            guard entry.status == .failed || entry.status == .recoverableInconsistency else {
                return
            }
            entry.status = .pending
            entry.acceptedByServer = false
            entry.acceptedAt = nil
            entry.lastError = nil
        }
    }

    /// DismissLocalMessage.
    func dismiss(_ localId: String) async {
        guard let idx = entries.firstIndex(where: { $0.localId == localId }) else { return }
        entries[idx].status = .dismissed
        _ = await persist()
    }

    /// Hide a local bubble as soon as authoritative history reflects it.
    /// This is deliberately memory-only: durable reconciliation still waits
    /// until the matching conversation snapshot is safely on disk.
    func suppress(authoritativeMessageIds: Set<String>) {
        suppressedMessageIds.formUnion(authoritativeMessageIds)
    }
    /// AuthoritativeMessageReconcilesQueueEntry: an entry reflected by the
    /// server's exact or conversation-scoped canonical identity is done.
    /// Applies to fresh sends, steering-queued sends, and rehydrated
    /// entries after an app restart alike.
    func reconcile(authoritativeMessageIds: Set<String>) {
        var changed = false
        for idx in entries.indices {
            let entry = entries[idx]
            if entry.isVisible && entry.isReflected(in: authoritativeMessageIds) {
                entries[idx].status = .reconciled
                changed = true
            }
        }
        if changed { persistEventually() }
    }

    /// AcceptedButCausallyProvenMissingBecomesRecoverable, approximated by
    /// time: a non-steering entry the server accepted that still hasn't
    /// appeared in history after `window` seconds is surfaced with a retry
    /// affordance rather than left spinning forever. The window runs from
    /// acceptance, not composition. Steering-queued entries are exempt —
    /// they legitimately wait for the current turn to finish.
    func surfaceStaleAcceptedEntries(window: TimeInterval = 60) {
        var changed = false
        let cutoff = Date().addingTimeInterval(-window)
        for idx in entries.indices {
            let entry = entries[idx]
            if entry.acceptedByServer,
               entry.status == .pending,
               (entry.acceptedAt ?? entry.createdAt) < cutoff {
                entries[idx].status = .recoverableInconsistency
                changed = true
            }
        }
        if changed { persistEventually() }
    }
}
