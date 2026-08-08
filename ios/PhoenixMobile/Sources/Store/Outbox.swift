import Foundation
import Observation

/// A locally-authored message that has not yet been confirmed by
/// authoritative server history. Implements the client-side delivery
/// contract in specs/user_message_queue/user_message_queue.allium:
/// `localId` doubles as the POST `message_id`, so retries are idempotent
/// and reconciliation joins that submitted identity to server history.
struct OutboxEntry: Codable, Identifiable, Equatable {
    enum Status: String, Codable {
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
    let conversationId: String
    private(set) var entries: [OutboxEntry] = []
    /// False when the last disk write failed (storage full/unavailable).
    /// Queued entries then exist in memory only — the UI warns that they
    /// won't survive an app restart. Cleared by the next successful write.
    private(set) var persistenceHealthy = true

    /// Bump when OutboxEntry's persisted shape changes incompatibly, and
    /// add a migrate branch below (DiskStore versioning rule). v1 is the
    /// full current shape; pre-envelope legacy files load as bare payload.
    static let schemaVersion = 1

    private var storeName: String { "outbox-\(conversationId)" }

    init(conversationId: String) {
        self.conversationId = conversationId
        // Rehydrate only entries tagged with this conversation — a foreign
        // entry can never reconcile here and must not render (spec rule
        // RehydrateQueueForConversationOnly).
        let loaded = DiskStore.loadVersioned(
            [OutboxEntry].self, name: storeName, version: Self.schemaVersion) ?? []
        entries = loaded.filter { $0.conversationId == conversationId && $0.isVisible }
    }

    var visibleEntries: [OutboxEntry] {
        entries.filter(\.isVisible)
    }

    var hasSendableEntries: Bool {
        entries.contains { $0.status == .pending && !$0.acceptedByServer }
    }

    private func persist() {
        // Terminal entries are pruned at persistence time; they carry no
        // future obligation.
        persistenceHealthy = DiskStore.saveVersioned(
            entries.filter(\.isVisible), name: storeName, version: Self.schemaVersion)
    }

    /// Re-establish the enqueue-before-POST durability point immediately
    /// before delivery. A transiently failed enqueue write can recover here;
    /// a continuing failure keeps every entry unsendable.
    func prepareForDelivery() -> Bool {
        persist()
        return persistenceHealthy
    }

    /// A hard-deleted conversation owns no remaining local delivery state.
    func clear() {
        entries.removeAll()
        persistenceHealthy = true
        DiskStore.remove(name: storeName)
    }

    private func update(_ localId: String, _ mutate: (inout OutboxEntry) -> Void) {
        guard let idx = entries.firstIndex(where: { $0.localId == localId }) else { return }
        mutate(&entries[idx])
        persist()
    }

    // MARK: - Contract transitions

    /// EnqueueLocalMessage: the entry exists (and persists) before any POST
    /// is attempted, so navigation or connection loss cannot erase the
    /// user's words.
    func enqueue(text: String, images: [ImagePayload] = []) -> OutboxEntry {
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
        persist()
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
    func dismiss(_ localId: String) {
        update(localId) { entry in
            entry.status = .dismissed
        }
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
        if changed { persist() }
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
        if changed { persist() }
    }
}
