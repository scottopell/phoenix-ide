import Foundation
import Observation

protocol SessionTiming: Sendable {
    func sleep(seconds: TimeInterval) async throws
}

struct LiveSessionTiming: SessionTiming {
    func sleep(seconds: TimeInterval) async throws {
        try await Task.sleep(for: .seconds(seconds))
    }
}

typealias ConversationEventStreamOpener = @Sendable (PhoenixAPI, String) async throws -> AsyncThrowingStream<PhoenixEvent, Error>

@Sendable
func defaultConversationEventStreamOpener(
    api: PhoenixAPI,
    conversationId: String
) async throws -> AsyncThrowingStream<PhoenixEvent, Error> {
    let (bytes, _) = try await api.openStream(conversationId: conversationId)
    return AsyncThrowingStream { continuation in
        let task = Task {
            do {
                for try await event in ConversationSession.decodedEvents(from: bytes) {
                    continuation.yield(event)
                }
                continuation.finish()
            } catch {
                continuation.finish(throwing: error)
            }
        }
        continuation.onTermination = { @Sendable _ in task.cancel() }
    }
}

/// Live model for one open conversation: cached snapshot + SSE reducer +
/// outbox. Owns the stream lifecycle (connect, reconnect with backoff,
/// resync via init snapshots) and drains the outbox whenever sending might
/// newly succeed (connectivity restored, stream reconnected, app foregrounded).
@MainActor
@Observable
final class ConversationSession {
    enum ConnectionState: Equatable {
        case idle
        case connecting
        case live
        /// Disconnected; next attempt at the associated time.
        case waitingToRetry(nextAttempt: Date)
        case offline
    }

    let conversationId: String
    let outbox: Outbox

    private(set) var conversation: Conversation?
    private(set) var messages: [Message] = []
    private(set) var agentWorking = false
    private(set) var presentationMode: String?
    var convState: JSONValue? { conversation?.state }
    var typedState: ConversationState { ConversationState.parse(conversation?.state) }
    private(set) var connection: ConnectionState = .idle
    /// In-flight LLM text accumulated from token events; cleared when the
    /// finalized message arrives or the turn ends.
    private(set) var streamingText = ""
    private(set) var lastErrorToast: String?
    private(set) var isHardDeleted = false
    private(set) var isHardDeletePending = false
    private(set) var isArchiving = false
    var acceptsChatMessage: Bool {
        acceptsConversationActions && typedState.acceptsChatMessage
    }
    var acceptsConversationActions: Bool {
        guard case .legacyReadOnly = hydrationAuthority else {
            return !isHardDeleted && !isHardDeletePending && !isArchiving && conversation?.archived != true
        }
        return false
    }
    /// tool_use_id -> the invoking block's tool name + input. Lets a tool
    /// result message (which carries only `tool_use_id`) find its native
    /// renderer. Rebuilt on message changes, not per render.
    private(set) var toolUseIndex: [String: ToolUseRef] = [:]

    private var lastSequenceId: Int64 = 0
    private var streamingRequestId: String?
    private var api: PhoenixAPI
    private let connectivity: ConnectivityMonitor
    private var connectivityToken: UUID?
    private var streamTask: Task<Void, Never>?
    private var drainTask: Task<Void, Never>?
    private var drainGeneration = 0
    private var lastCompletedDrainGeneration = 0
    private var staleCheckTask: Task<Void, Never>?
    private var cancelNeedsAgentDoneFallback = false
    /// localIds with a POST in flight — prevents duplicate concurrent sends
    /// of one entry (resending a *different* entry is always safe).
    private var inFlight: Set<String> = []
    private var retryDelay: TimeInterval = 1
    private var outboxAuthorityGeneration = 0
    private var liveWorkGeneration = 0
    private let retryTiming: any SessionTiming
    private let staleCheckTiming: any SessionTiming
    private let openEventStream: ConversationEventStreamOpener
    private let deliveryTriggerAllowed: () -> Bool
    struct HardDeleteContext: Sendable {
        let conversationId: String
        let aggregateAuthority: String
        let configurationIdentity: APIConfigurationIdentity
    }

    private var onConversationUpdate: ((Conversation) -> Void)?
    private var onSessionEvent: ((ProductConversationSessionEvent) -> Void)?
    private var onHardDeleted: @MainActor (HardDeleteContext) async -> Void
    private var hardDeleteReportTask: Task<Void, Never>?
    private var viewIsActive = false
    private var replayFromPendingAnchor = false
    private var streamBlockedUntilConfigurationChange = false
    private let snapshotWriter: VersionedDiskWriter
    private var latestSnapshotRevision = 0
    private var pendingSnapshotConfigurationIdentity: APIConfigurationIdentity?
    private var snapshotPersistenceEnabled = true
    private var snapshotNeedsOutboxReconciliation = false
    private var snapshotNeedsOutboxDrain = false
    /// Init's persisted-message anchor. Live messages above it may be eager
    /// assistant output, so snapshot persistence excludes them until resync.
    private var durableMessageSequenceCeiling: Int64 = 0

    private struct PendingMessagePatch {
        var content: JSONValue?
        var displayData: JSONValue?
        var durationMs: Double?
    }

    /// `message_updated` can precede its eager `message` during replay.
    /// Retain the newest fields until the identity-bearing message arrives.
    private var pendingMessagePatches: [String: PendingMessagePatch] = [:]

    enum HydrationAuthority: Equatable {
        case none
        case legacyReadOnly(PersistedSnapshotAuthority?)
        case current(AuthoritativeSnapshotReceipt)
    }

    private(set) var hydrationAuthority: HydrationAuthority = .none
    var authoritativeSnapshotReceipt: AuthoritativeSnapshotReceipt? {
        guard case .current(let receipt) = hydrationAuthority else { return nil }
        return receipt
    }
    // MARK: - Persistence

    struct PersistedSnapshotAuthority: Codable, Equatable, Sendable {
        let configurationIdentity: APIConfigurationIdentity
        let aggregateAuthority: String
        let syncedAt: Date
    }

    private let aggregateAuthority: String

    private static func receiptIdentity(
        for conversation: Conversation,
        sessionConversationId: String,
        expectedAggregateAuthority: String
    ) -> (conversationId: String, aggregateId: String)? {
        guard conversation.id == sessionConversationId else { return nil }
        guard conversation.aggregateIdentity == expectedAggregateAuthority else { return nil }
        return (conversation.id, expectedAggregateAuthority)
    }

    private static func aggregateAuthority(conversationId: String, aggregateAuthority: String?) -> String {
        aggregateAuthority ?? conversationId
    }

    struct AuthoritativeSnapshotReceipt: Equatable, Sendable {

        let conversationId: String
        let aggregateId: String
        let configurationIdentity: APIConfigurationIdentity
        let revision: Int
        let syncedAt: Date
    }

    /// Bump when Snapshot's persisted shape changes incompatibly (DiskStore
    /// versioning rule). Additive-optional fields remain compatible.
    static let snapshotSchemaVersion = 1

    struct PersistedSnapshot: Codable, Sendable {
        var conversation: Conversation?
        var messages: [Message]
        var lastSequenceId: Int64
        /// Missing only in snapshots written before transcript generations
        /// were part of the iOS cache; nil forces replacement on next init.
        var transcriptGeneration: Int64?
        /// Missing in snapshots written before cache freshness was tracked.
        var syncedAt: Date?
        /// owned: snapshots written before authority scoping had no persisted
        /// authority metadata; rendering remains valid, but authoritative
        /// replay stays locked until a current authoritative init rewrites it.
        var authoritative: PersistedSnapshotAuthority?
    }

    private var transcriptGeneration: Int64?
    private(set) var snapshotSyncedAt: Date?
    private var pendingAuthoritativeSnapshot: PersistedSnapshotAuthority?

    init(
        conversationId: String,
        api: PhoenixAPI,
        connectivity: ConnectivityMonitor,
        outboxPersistence: OutboxPersistenceHandle,
        snapshotPersistence: VersionedDiskWriter,
        retryTiming: any SessionTiming,
        staleCheckTiming: any SessionTiming,
        openEventStream: @escaping ConversationEventStreamOpener = defaultConversationEventStreamOpener,
        deliveryTriggerAllowed: @escaping () -> Bool = { true },
        legacySnapshotPersistenceScope: PersistenceScopeIdentity? = nil,
        aggregateAuthority: String? = nil,
        onConversationUpdate: ((Conversation) -> Void)? = nil,
        onHardDeleted: @escaping @MainActor (HardDeleteContext) async -> Void = { _ in }
    ) {
        self.conversationId = conversationId
        self.api = api
        self.connectivity = connectivity
        self.retryTiming = retryTiming
        self.staleCheckTiming = staleCheckTiming
        self.openEventStream = openEventStream
        self.deliveryTriggerAllowed = deliveryTriggerAllowed
        self.aggregateAuthority = Self.aggregateAuthority(
            conversationId: conversationId,
            aggregateAuthority: aggregateAuthority)
        self.onConversationUpdate = onConversationUpdate
        self.onHardDeleted = onHardDeleted
        self.outbox = Outbox(
            conversationId: conversationId,
            aggregateAuthority: self.aggregateAuthority,
            persistenceScope: PersistenceScopeIdentity(
                serverURL: api.configurationIdentity.serverURL,
                credentialGeneration: api.configurationIdentity.credentialGeneration),
            persistence: outboxPersistence)
        self.snapshotWriter = snapshotPersistence

        // Cached snapshot renders immediately; the stream refreshes it.
        let loadedSnapshot: DiskStore.VersionedLoad<PersistedSnapshot> = DiskStore.loadVersionedResult(
            PersistedSnapshot.self,
            source: snapshotWriter.destinationURL,
            version: Self.snapshotSchemaVersion)
        if case .value(let snap) = loadedSnapshot,
           let persistedConversation = snap.conversation,
           let authority = snap.authoritative,
           authority.configurationIdentity == api.configurationIdentity,
           authority.aggregateAuthority == self.aggregateAuthority,
           let receiptIdentity = Self.receiptIdentity(
               for: persistedConversation,
               sessionConversationId: conversationId,
               expectedAggregateAuthority: self.aggregateAuthority)
        {
            conversation = persistedConversation
            messages = snap.messages
            durableMessageSequenceCeiling = snap.messages.map { $0.sequence_id }.max() ?? 0
            lastSequenceId = snap.lastSequenceId
            transcriptGeneration = snap.transcriptGeneration
            snapshotSyncedAt = snap.syncedAt
            hydrationAuthority = .current(AuthoritativeSnapshotReceipt(
                conversationId: receiptIdentity.conversationId,
                aggregateId: receiptIdentity.aggregateId,
                configurationIdentity: api.configurationIdentity,
                revision: 0,
                syncedAt: authority.syncedAt))
            replayFromPendingAnchor = true
            presentationMode = persistedConversation.presentation_mode
            // Busy flag follows the cached mode the same way live
            // state_change events derive it — a snapshot taken mid-turn
            // must not open looking idle.
            agentWorking = presentationMode == "working"
            rebuildToolUseIndex()
            // A prior crash can leave the authoritative snapshot durable but
            // the matching outbox row not yet pruned. Reconcile at load so the
            // same user message never renders twice while offline.
            reconcileOutbox()
        } else if case .value(let snap) = loadedSnapshot,
                  (snap.authoritative == nil
                    && legacySnapshotPersistenceScope == api.configurationIdentity.persistenceScope)
                    || snap.authoritative?.configurationIdentity.persistenceScope == api.configurationIdentity.persistenceScope,
                  let persistedConversation = snap.conversation,
                  Self.receiptIdentity(
                      for: persistedConversation,
                      sessionConversationId: conversationId,
                      expectedAggregateAuthority: self.aggregateAuthority) != nil
        {
            conversation = persistedConversation
            messages = snap.messages
            durableMessageSequenceCeiling = snap.messages.map { $0.sequence_id }.max() ?? 0
            lastSequenceId = snap.lastSequenceId
            transcriptGeneration = snap.transcriptGeneration
            snapshotSyncedAt = snap.syncedAt
            presentationMode = persistedConversation.presentation_mode
            agentWorking = presentationMode == "working"
            rebuildToolUseIndex()
            hydrationAuthority = .legacyReadOnly(snap.authoritative)
        }
    }

    func start() {
        guard !isHardDeleted else { return }
        viewIsActive = true
        if connectivityToken == nil {
            connectivityToken = connectivity.addPathObserver(
                onRestore: { [weak self] in self?.connectivityRestored() },
                onLoss: { [weak self] in self?.connectivityLost() })
        }
        resumeLiveTasks()
    }

    func replaceAPI(_ api: PhoenixAPI) {
        invalidateOutboxAuthority()
        invalidateLiveWork()
        inFlight.removeAll()
        self.api = api
        pendingSnapshotConfigurationIdentity = nil
        pendingAuthoritativeSnapshot = nil
        hydrationAuthority = .none
        streamBlockedUntilConfigurationChange = false
        if viewIsActive {
            streamTask?.cancel()
            streamTask = nil
            connection = .idle
            resumeLiveTasks()
        }
        drainOutbox()
    }

    func revokeForHardDelete() {
        isHardDeleted = true
        isHardDeletePending = false
        snapshotPersistenceEnabled = false
        invalidateOutboxAuthority()
        conversation = nil
        messages = []
        durableMessageSequenceCeiling = 0
        presentationMode = "done"
        agentWorking = false
        streamingText = ""
        streamingRequestId = nil
        pendingMessagePatches.removeAll()
        toolUseIndex = [:]
        hydrationAuthority = .none
        stop()
    }

    func invalidateForAggregateReplacement() {
        invalidateOutboxAuthority()
        invalidateLiveWork()
        hydrationAuthority = .none
        stop()
    }

    func invalidateConfiguration() {
        invalidateOutboxAuthority()
        invalidateLiveWork()
        streamBlockedUntilConfigurationChange = true
        streamTask?.cancel()
        streamTask = nil
        staleCheckTask?.cancel()
        staleCheckTask = nil
        inFlight.removeAll()
        drainGeneration &+= 1
        drainTask?.cancel()
        drainTask = nil
        pendingSnapshotConfigurationIdentity = nil
        pendingAuthoritativeSnapshot = nil
        hydrationAuthority = .none
        actionAttempt = nil
        connection = .idle
        onSessionEvent?(.connectionChanged(.idle))
    }

    func setSessionEventObserver(_ observer: ((ProductConversationSessionEvent) -> Void)?) {
        onSessionEvent = observer
    }

    func adoptOpenOwnership(
        onConversationUpdate: @escaping (Conversation) -> Void,
        onHardDeleted: @escaping @MainActor (HardDeleteContext) async -> Void
    ) {
        self.onConversationUpdate = onConversationUpdate
        self.onHardDeleted = onHardDeleted
    }

    func stop() {
        viewIsActive = false
        invalidateLiveWork()
        pauseLiveTasks()
        if let token = connectivityToken {
            connectivity.removePathObserver(token)
            connectivityToken = nil
        }
    }

    /// End the opened-view stream while retaining this session as the owner
    /// of its disk-backed outbox.
    func closeView() {
        viewIsActive = false
        invalidateLiveWork()
        pauseLiveTasks()
    }

    /// Background suspension preserves whether the view is open so a later
    /// foreground transition resumes only that conversation's live stream.
    func pauseForBackground() {
        invalidateLiveWork()
        pauseLiveTasks()
    }

    private func pauseLiveTasks() {
        streamTask?.cancel()
        streamTask = nil
        staleCheckTask?.cancel()
        staleCheckTask = nil
        connection = .idle
        onSessionEvent?(.connectionChanged(.idle))
        persistSnapshot()
    }

    private func invalidateOutboxAuthority() {
        outboxAuthorityGeneration &+= 1
    }

    private func invalidateLiveWork() {
        liveWorkGeneration &+= 1
    }

    private func isCurrentLiveWork(_ generation: Int, apiIdentity: APIConfigurationIdentity) -> Bool {
        !Task.isCancelled && generation == liveWorkGeneration && viewIsActive
            && apiIdentity == api.configurationIdentity && !isHardDeleted
    }

    private func resumeLiveTasks() {
        guard viewIsActive, !isHardDeleted,
              !streamBlockedUntilConfigurationChange
        else { return }
        let generation = liveWorkGeneration
        let apiIdentity = api.configurationIdentity
        if streamTask == nil {
            streamTask = Task { await streamLoop(generation: generation, apiIdentity: apiIdentity) }
        }
        if staleCheckTask == nil {
            staleCheckTask = Task { await staleCheckLoop(generation: generation, apiIdentity: apiIdentity) }
        }
    }

    /// Called on scenePhase -> .active: the stream task was likely torn down
    /// while backgrounded; restart it and drain anything queued.
    func resyncAfterForeground() {
        guard !isHardDeleted, deliveryTriggerAllowed() else { return }
        resumeLiveTasks()
        drainOutbox()
    }

    func resyncAfterConnectivityRestore() {
        connectivityRestored()
    }

    private func connectivityRestored() {
        guard !isHardDeleted, deliveryTriggerAllowed() else { return }
        if viewIsActive, !streamBlockedUntilConfigurationChange {
            streamTask?.cancel()
            streamTask = nil
            resumeLiveTasks()
        }
        drainOutbox()
    }

    private func connectivityLost() {
        invalidateLiveWork()
        streamTask?.cancel()
        streamTask = nil
        staleCheckTask?.cancel()
        staleCheckTask = nil
        connection = .offline
        onSessionEvent?(.connectionChanged(.offline))
    }

    private func snapshotForPersistence(authoritative: Bool) -> PersistedSnapshot {
        let authority: PersistedSnapshotAuthority?
        let syncedAt: Date?
        if authoritative {
            let now = Date()
            let currentAuthority = PersistedSnapshotAuthority(
                configurationIdentity: api.configurationIdentity,
                aggregateAuthority: aggregateAuthority,
                syncedAt: now)
            pendingAuthoritativeSnapshot = currentAuthority
            authority = currentAuthority
            syncedAt = now
        } else if let pendingAuthoritativeSnapshot,
                  pendingAuthoritativeSnapshot.configurationIdentity == api.configurationIdentity
        {
            authority = pendingAuthoritativeSnapshot
            syncedAt = pendingAuthoritativeSnapshot.syncedAt
        } else if let receipt = authoritativeSnapshotReceipt,
                  receipt.configurationIdentity == api.configurationIdentity
        {
            authority = PersistedSnapshotAuthority(
                configurationIdentity: receipt.configurationIdentity,
                aggregateAuthority: receipt.aggregateId,
                syncedAt: receipt.syncedAt)
            syncedAt = receipt.syncedAt
        } else if case .legacyReadOnly(let persistedAuthority) = hydrationAuthority,
                  let persistedAuthority,
                  persistedAuthority.configurationIdentity.persistenceScope == api.configurationIdentity.persistenceScope
        {
            authority = persistedAuthority
            syncedAt = persistedAuthority.syncedAt
        } else {
            authority = nil
            syncedAt = snapshotSyncedAt
        }
        return PersistedSnapshot(
            conversation: conversation,
            messages: Self.durableMessages(
                messages, through: durableMessageSequenceCeiling),
            lastSequenceId: lastSequenceId,
            transcriptGeneration: transcriptGeneration,
            syncedAt: syncedAt,
            authoritative: authority)
    }

    private func persistSnapshot(
        authoritative: Bool = false,
        reconcileOutboxOnSuccess: Bool = false,
        drainOutboxAfter: Bool = false
    ) {
        guard !isHardDeleted, snapshotPersistenceEnabled else { return }
        snapshotNeedsOutboxReconciliation =
            snapshotNeedsOutboxReconciliation || reconcileOutboxOnSuccess
        snapshotNeedsOutboxDrain = snapshotNeedsOutboxDrain || drainOutboxAfter
        let snapshot = snapshotForPersistence(authoritative: authoritative)
        let configurationIdentity = api.configurationIdentity
        let revision = snapshotWriter.reserveRevision()
        latestSnapshotRevision = revision
        pendingSnapshotConfigurationIdentity = configurationIdentity
        Task { [weak self, snapshotWriter] in
            let didSave = await snapshotWriter.save(snapshot, revision: revision)
            self?.completeSnapshotPersistence(
                snapshot,
                revision: revision,
                didSave: didSave,
                configurationIdentity: configurationIdentity)
        }
    }

    @discardableResult
    private func completeSnapshotPersistence(
        _ snapshot: PersistedSnapshot,
        revision: Int,
        didSave: Bool,
        configurationIdentity: APIConfigurationIdentity
    ) -> Bool {
        guard latestSnapshotRevision == revision,
              pendingSnapshotConfigurationIdentity == configurationIdentity,
              api.configurationIdentity == configurationIdentity,
              snapshotPersistenceEnabled,
              !isHardDeleted
        else { return false }
        if didSave {
            snapshotSyncedAt = snapshot.syncedAt
            pendingAuthoritativeSnapshot = nil
            pendingSnapshotConfigurationIdentity = nil
            if let persistedConversation = snapshot.conversation,
               let authority = snapshot.authoritative,
               authority.aggregateAuthority == aggregateAuthority,
               let receiptIdentity = Self.receiptIdentity(
                   for: persistedConversation,
                   sessionConversationId: conversationId,
                   expectedAggregateAuthority: self.aggregateAuthority)
            {
                hydrationAuthority = .current(AuthoritativeSnapshotReceipt(
                    conversationId: receiptIdentity.conversationId,
                    aggregateId: receiptIdentity.aggregateId,
                    configurationIdentity: authority.configurationIdentity,
                    revision: revision,
                    syncedAt: authority.syncedAt))
            }
            if snapshotNeedsOutboxReconciliation {
                snapshotNeedsOutboxReconciliation = false
                reconcileOutbox()
            }
        }
        if snapshotNeedsOutboxDrain {
            snapshotNeedsOutboxDrain = false
            drainOutbox()
        }
        return didSave
    }

    @discardableResult
    func flushSnapshotPersistence() async -> Bool {
        guard !isHardDeleted, snapshotPersistenceEnabled else { return false }
        let snapshot = snapshotForPersistence(authoritative: false)
        let configurationIdentity = api.configurationIdentity
        let revision = snapshotWriter.reserveRevision()
        latestSnapshotRevision = revision
        pendingSnapshotConfigurationIdentity = configurationIdentity
        let didSave = await snapshotWriter.save(snapshot, revision: revision)
        return completeSnapshotPersistence(
            snapshot,
            revision: revision,
            didSave: didSave,
            configurationIdentity: configurationIdentity)
    }

    func currentDrainTaskForTesting() -> Task<Void, Never>? {
        drainTask
    }

    func currentStreamTaskForTesting() -> Task<Void, Never>? {
        streamTask
    }

    func awaitHardDeleteReportForTesting() async {
        await hardDeleteReportTask?.value
    }

    func clearCachedSnapshotAndWait() async {
        invalidateOutboxAuthority()
        snapshotPersistenceEnabled = false
        snapshotNeedsOutboxReconciliation = false
        snapshotNeedsOutboxDrain = false
        pendingSnapshotConfigurationIdentity = nil
        pendingAuthoritativeSnapshot = nil
        drainGeneration &+= 1
        drainTask?.cancel()
        drainTask = nil
        hydrationAuthority = .none
        let revision = snapshotWriter.reserveRevision()
        latestSnapshotRevision = revision
        await snapshotWriter.remove(revision: revision)
    }

    // MARK: - Sending

    var canSendPersistedOutbox: Bool {
        !isHardDeletePending
            && authoritativeSnapshotReceipt?.configurationIdentity == api.configurationIdentity
    }

    // MARK: - Sending

    /// Optimistic enqueue-then-send. The entry is persisted before the POST
    /// leaves the device; if the network is down the send is deferred, not
    /// failed. Images ride the same outbox path as text — same durability,
    /// same idempotent delivery.
    @discardableResult
    func send(text: String, images: [ImagePayload] = []) async -> Bool {
        guard !isHardDeleted else { return false }
        guard ClientOperation.chat.policy == .outboxed else { return false }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (!trimmed.isEmpty || !images.isEmpty), acceptsChatMessage else {
            return false
        }
        guard await outbox.enqueue(text: trimmed, images: images) != nil else {
            lastErrorToast = "Message could not be saved on this device. Free storage and try again."
            onSessionEvent?(.errorToastChanged(lastErrorToast))
            return false
        }
        drainOutbox()
        onSessionEvent?(.outboxChanged)
        return true
    }

    func retryEntry(_ localId: String) {
        outbox.retry(localId)
        drainOutbox()
        onSessionEvent?(.outboxChanged)
    }

    func dismissEntry(_ localId: String) async {
        await outbox.dismiss(localId)
        onSessionEvent?(.outboxChanged)
    }

    func beginArchiving() -> Bool {
        guard !isArchiving, outbox.visibleEntries.isEmpty else { return false }
        isArchiving = true
        return true
    }

    func endArchiving() {
        isArchiving = false
    }

    /// Attempt delivery of every sendable entry, oldest first. Safe to call
    /// eagerly and repeatedly: entry-level `inFlight` guards duplicate
    /// concurrent POSTs, and the server's message_id idempotency makes
    /// genuine resends no-ops.
    @discardableResult
    func drainOutbox() -> Int? {
        guard !isHardDeleted, deliveryTriggerAllowed() else { return nil }
        if drainTask != nil {
            return drainGeneration
        }
        drainGeneration &+= 1
        let generation = drainGeneration
        drainTask = Task {
            defer {
                if drainGeneration == generation,
                   authoritativeSnapshotReceipt?.configurationIdentity == api.configurationIdentity,
                   !isHardDeleted
                {
                    lastCompletedDrainGeneration = max(lastCompletedDrainGeneration, generation)
                }
                if drainGeneration == generation {
                    drainTask = nil
                }
            }
            // Loop until no sendable entries remain, so a message enqueued
            // while a drain is already running is picked up by this pass
            // instead of waiting for the next trigger.
            while !Task.isCancelled {
                // Never POST an entry whose durable copy is missing. This
                // retries the persistence point on every delivery trigger.
                let authorityGeneration = outboxAuthorityGeneration
                let configurationIdentity = api.configurationIdentity
                guard authoritativeSnapshotReceipt?.configurationIdentity == configurationIdentity,
                      await outbox.prepareForDelivery(),
                      authorityGeneration == outboxAuthorityGeneration,
                      configurationIdentity == api.configurationIdentity,
                      authoritativeSnapshotReceipt?.configurationIdentity == configurationIdentity,
                      !isHardDeleted
                else { return }
                let sendable = outbox.entries.filter {
                    $0.status == .pending && !$0.acceptedByServer
                        && !inFlight.contains($0.localId)
                }
                guard let entry = sendable.first else { return }
                inFlight.insert(entry.localId)
                defer { inFlight.remove(entry.localId) }
                outbox.markAttempted(entry.localId)
                guard authorityGeneration == outboxAuthorityGeneration,
                      configurationIdentity == api.configurationIdentity,
                      authoritativeSnapshotReceipt?.configurationIdentity == configurationIdentity,
                      !isHardDeleted
                else { return }
                do {
                    let response = try await api.sendChat(
                        conversationId: conversationId,
                        text: entry.text,
                        images: entry.images,
                        messageId: entry.localId)
                    guard authorityGeneration == outboxAuthorityGeneration,
                          configurationIdentity == self.api.configurationIdentity,
                          !isHardDeleted
                    else { return }
                    outbox.markAccepted(entry.localId, steering: response.steering ?? false)
                    if response.already_persisted == true {
                        await reconcileAlreadyPersisted(entry.localId)
                    }
                } catch let error as APIError where error.isRetryableChatDeliveryFailure {
                    guard authorityGeneration == outboxAuthorityGeneration,
                          configurationIdentity == self.api.configurationIdentity,
                          !isHardDeleted
                    else { return }
                    return
                } catch {
                    guard authorityGeneration == outboxAuthorityGeneration,
                          configurationIdentity == self.api.configurationIdentity,
                          !isHardDeleted,
                          !Task.isCancelled
                    else { return }
                    outbox.markFailed(
                        entry.localId,
                        error: (error as? APIError)?.errorDescription
                            ?? error.localizedDescription)
                }
            }
        }
        return generation
    }

    #if DEBUG
    var aggregateAuthorityForTesting: String { aggregateAuthority }

    func currentDrainGenerationForTesting() -> Int? {
        drainTask == nil ? nil : drainGeneration
    }
    #endif

    func awaitDrainOutbox(generation: Int) async -> Bool {
        if lastCompletedDrainGeneration >= generation {
            return true
        }
        if generation != drainGeneration {
            return false
        }
        while generation == drainGeneration {
            guard let drainTask else { return false }
            await drainTask.value
            if lastCompletedDrainGeneration >= generation {
                return true
            }
        }
        return false
    }

    /// The action currently being executed, or nil. Views use this to
    /// disable controls and show progress — approval buttons especially
    /// must not double-fire.
    private struct ActionAttempt {
        let action: ConversationAction
        let originState: ConversationState?
        let token: UUID
    }
    private var actionAttempt: ActionAttempt?
    var actionInFlight: ConversationAction? { actionAttempt?.action }

    /// Execute a session-scoped action per its declared delivery policy
    /// (ConversationAction). Online-only actions fail fast with a toast
    /// when offline — deliberately not queued, see the policy doc.
    func perform(_ action: ConversationAction) {
        guard acceptsConversationActions, actionAttempt == nil else { return }
        switch ClientOperation.conversationAction(action).policy {
        case .onlineOnly:
            guard connectivity.isOnline else {
                lastErrorToast = "This action needs a connection — it can't be queued."
                onSessionEvent?(.errorToastChanged(lastErrorToast))
                return
            }
        case .outboxed:
            break  // never blocked on connectivity by definition
        }
        let token = UUID()
        actionAttempt = ActionAttempt(
            action: action,
            originState: typedState,
            token: token)
        Task {
            do {
                switch action {
                case .cancel:
                    _ = try await api.cancel(conversationId: conversationId)
                case .dismissError:
                    try await api.dismissError(conversationId: conversationId)
                case .approveTask(let handoff):
                    try await api.approveTask(
                        conversationId: conversationId, handoff: handoff)
                case .rejectTask:
                    try await api.rejectTask(conversationId: conversationId)
                case .provideTaskFeedback(let feedback):
                    try await api.sendTaskFeedback(
                        conversationId: conversationId, annotations: feedback.text)
                case .respondToQuestions(let answers):
                    try await api.respondToQuestion(
                        conversationId: conversationId, answers: answers)
                case .dismissQuestion:
                    try await api.dismissQuestion(conversationId: conversationId)
                }
            } catch {
                guard actionAttempt?.token == token else { return }
                if case .cancel = action {
                    cancelNeedsAgentDoneFallback = false
                }
                actionAttempt = nil
                lastErrorToast = (error as? APIError)?.errorDescription
                    ?? error.localizedDescription
                onSessionEvent?(.errorToastChanged(lastErrorToast))
            }
        }
    }

    func clearErrorToast() {
        lastErrorToast = nil
        onSessionEvent?(.errorToastChanged(nil))
    }

    // MARK: - Stream lifecycle

    /// URLSession.AsyncBytes is intentionally parsed in a detached producer.
    /// ConversationSession is MainActor-isolated, so iterating and JSON-
    /// decoding a multi-megabyte init here directly would freeze input and
    /// scrolling. Only decoded events cross back to the reducer.
    fileprivate nonisolated static func decodedEvents(
        from bytes: URLSession.AsyncBytes
    ) -> AsyncThrowingStream<PhoenixEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task.detached(priority: .utility) {
                do {
                    var parser = SSEParser()
                    for try await byte in bytes {
                        if Task.isCancelled { break }
                        if let frame = parser.consume(byte),
                           let event = PhoenixEvent.decode(frame: frame) {
                            continuation.yield(event)
                        }
                    }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { @Sendable _ in task.cancel() }
        }
    }

    private func streamLoop(generation: Int, apiIdentity: APIConfigurationIdentity) async {
        retryDelay = 1
        while !Task.isCancelled {
            guard isCurrentLiveWork(generation, apiIdentity: apiIdentity) else { return }
            if !connectivity.isOnline {
                connection = .offline
                onSessionEvent?(.connectionChanged(.offline))
                return
            }

            connection = .connecting
            onSessionEvent?(.connectionChanged(.connecting))
            do {
                let events = try await openEventStream(api, conversationId)
                guard isCurrentLiveWork(generation, apiIdentity: apiIdentity) else { return }
                connection = .live
                onSessionEvent?(.connectionChanged(.live))
                for try await event in events {
                    guard isCurrentLiveWork(generation, apiIdentity: apiIdentity) else { return }
                    receive(event)
                }
            } catch is CancellationError {
                return
            } catch let error as APIError {
                guard isCurrentLiveWork(generation, apiIdentity: apiIdentity) else { return }
                if error.isNotFound {
                    handleHardDeletion()
                    return
                }
                if case .certificatePinMismatch = error {
                    lastErrorToast = error.errorDescription
                    onSessionEvent?(.errorToastChanged(lastErrorToast))
                    connection = .idle
                    onSessionEvent?(.connectionChanged(.idle))
                    return
                }
                if error.isPermanentStreamAuthenticationFailure {
                    streamBlockedUntilConfigurationChange = true
                    lastErrorToast = error.errorDescription
                    onSessionEvent?(.errorToastChanged(lastErrorToast))
                    connection = .idle
                    onSessionEvent?(.connectionChanged(.idle))
                    return
                }
            } catch {
                guard isCurrentLiveWork(generation, apiIdentity: apiIdentity) else { return }
            }

            guard isCurrentLiveWork(generation, apiIdentity: apiIdentity) else { return }
            persistSnapshot()
            let jitter = Double.random(in: 0...0.3) * retryDelay
            connection = .waitingToRetry(nextAttempt: Date().addingTimeInterval(retryDelay + jitter))
            onSessionEvent?(.connectionChanged(connection))
            do {
                try await retryTiming.sleep(seconds: retryDelay + jitter)
            } catch is CancellationError {
                return
            } catch {
                return
            }
            guard isCurrentLiveWork(generation, apiIdentity: apiIdentity) else { return }
            retryDelay = min(retryDelay * 2, 30)
        }
    }

    private func staleCheckLoop(generation: Int, apiIdentity: APIConfigurationIdentity) async {
        while !Task.isCancelled {
            do {
                try await staleCheckTiming.sleep(seconds: 20)
            } catch is CancellationError {
                return
            } catch {
                return
            }
            guard isCurrentLiveWork(generation, apiIdentity: apiIdentity) else { return }
            if connection == .live {
                outbox.surfaceStaleAcceptedEntries()
            }
        }
    }

    // MARK: - Reducer

    func receive(_ event: PhoenixEvent) {
        guard !isHardDeleted else { return }
        switch event {
        case .initSnapshot(let snap):
            retryDelay = 1
            let previousSequenceFloor = lastSequenceId
            let previousConversation = conversation
            let previousTypedState = typedState
            let generationMatches = transcriptGeneration == snap.transcriptGeneration
            let mustReplayFromAnchor = replayFromPendingAnchor
            replayFromPendingAnchor = false
            conversation = snap.conversation
            conversation?.transcript_generation = snap.transcriptGeneration
            conversation?.presentation_mode = snap.presentationMode
            if let mode = snap.presentationMode {
                conversation?.requires_action = mode == "needs_action"
            }
            cancelNeedsAgentDoneFallback = false
            clearResolvedActionIfStateAdvanced(currentState: typedState)
            messages = Self.reconcileTranscript(
                existing: messages,
                incoming: snap.messages,
                coverage: snap.transcriptCoverage,
                generationMatches: generationMatches)
            transcriptGeneration = snap.transcriptGeneration
            durableMessageSequenceCeiling = Self.durableCeilingAfterInit(
                anchor: snap.pendingAnchorSequenceId,
                messages: snap.messages)
            agentWorking = snap.agentWorking
            presentationMode = snap.presentationMode
            if mustReplayFromAnchor || previousSequenceFloor == 0 || !generationMatches
                || !snap.agentWorking || snap.pendingTruncated {
                streamingText = ""
                streamingRequestId = nil
            }
            pendingMessagePatches.removeAll()
            // Replay the ring through the same reducer so an in-flight turn
            // (streaming text, tool phase) survives the reconnect. The
            // replay floor is the ring anchor — ring entries sit in
            // (anchor, last_sequence_id], so anchoring at the tip would
            // silently drop the whole replay.
            lastSequenceId = Self.replayFloor(
                previous: previousSequenceFloor,
                anchor: snap.pendingAnchorSequenceId,
                serverTip: snap.lastSequenceId,
                generationMatches: generationMatches,
                restoredFromDisk: mustReplayFromAnchor)
            if !snap.pendingTruncated {
                for entry in snap.pendingEvents {
                    if let pending = PhoenixEvent.decode(pendingEntry: entry) {
                        applyLive(pending)
                    }
                }
            }
            lastSequenceId = max(lastSequenceId, snap.lastSequenceId)
            rebuildToolUseIndex()
            onSessionEvent?(.messagesChanged)
            if let conversation {
                onConversationUpdate?(conversation)
                emitAggregateTopologyInvalidationIfNeeded(
                    previousConversation: previousConversation,
                    previousState: previousTypedState,
                    conversation: conversation)
            }
            // Persist the authoritative snapshot BEFORE reconciling: the
            // outbox prune must never become durable while the message
            // snapshot that justifies it is still memory-only — a crash
            // between the two writes would lose the user's text from both.
            outbox.suppress(authoritativeMessageIds: Set(snap.messages.map(\.message_id)))
            persistSnapshot(
                authoritative: true,
                reconcileOutboxOnSuccess: true,
                drainOutboxAfter: true)

        default:
            applyLive(event)
        }
    }

    /// Sequence-guarded application of non-init events. Events at or below
    /// the current floor were already absorbed via a snapshot — drop them.
    private func applyLive(_ event: PhoenixEvent) {
        switch event {
        case .initSnapshot:
            return  // handled by receive()

        case .message(let seq, let message):
            guard applyIfNewer(seq) else { return }
            upsert(message)
            durableMessageSequenceCeiling = Self.durableCeilingAfterLiveMessage(
                current: durableMessageSequenceCeiling,
                message: message)
            outbox.suppress(authoritativeMessageIds: [message.message_id])
            if let createdAt = message.created_at,
               let messageDate = message.createdAtDate,
               var conversation,
               conversation.updatedAtDate.map({ messageDate > $0 }) ?? true {
                let previousConversation = self.conversation
                let previousState = typedState
                conversation.updated_at = createdAt
                self.conversation = conversation
                onConversationUpdate?(conversation)
                emitAggregateTopologyInvalidationIfNeeded(
                    previousConversation: previousConversation,
                    previousState: previousState,
                    conversation: conversation)
            }
            if message.message_type == "agent" {
                streamingText = ""
                streamingRequestId = nil
                rebuildToolUseIndex()
                onSessionEvent?(.messagesChanged)
            }
            // Snapshot before outbox prune — see the init branch.
            persistSnapshot(authoritative: true, reconcileOutboxOnSuccess: true)

        case .messageUpdated(
            let seq, let messageId, let content, let displayData, let durationMs,
            let updatedGeneration):
            // Stale guard applies here too: a replayed update from before
            // the floor must not clobber content a newer update already set.
            guard applyIfNewer(seq) else { return }
            if let updatedGeneration {
                transcriptGeneration = updatedGeneration
                conversation?.transcript_generation = updatedGeneration
            }
            guard let idx = messages.firstIndex(where: { $0.message_id == messageId }) else {
                var patch = pendingMessagePatches[messageId]
                    ?? PendingMessagePatch(content: nil, displayData: nil, durationMs: nil)
                if let content, content != .null { patch.content = content }
                if let displayData, displayData != .null {
                    patch.displayData = Self.mergeDisplayData(
                        existing: patch.displayData,
                        patch: displayData)
                }
                if let durationMs { patch.durationMs = durationMs }
                pendingMessagePatches[messageId] = patch
                return
            }
            if let content, content != .null { messages[idx].content = content }
            if let displayData, displayData != .null {
                messages[idx].display_data = Self.mergeDisplayData(
                    existing: messages[idx].display_data,
                    patch: displayData)
            }
            if let durationMs {
                messages[idx].display_data = Self.mergeDisplayData(
                    existing: messages[idx].display_data,
                    patch: .object(["duration_ms": .number(durationMs)]))
            }
            if let updatedGeneration {
                transcriptGeneration = updatedGeneration
                conversation?.transcript_generation = updatedGeneration
            }
            if messages[idx].message_type == "agent" { rebuildToolUseIndex() }
            persistSnapshot(authoritative: true)

        case .stateChange(let seq, let state, let mode, let stateUpdatedAt):
            guard applyIfNewer(seq) else { return }
            cancelNeedsAgentDoneFallback = false
            if let mode { presentationMode = mode }
            if var conversation {
                let previousConversation = self.conversation
                let previousState = typedState
                conversation.state = state
                if let stateUpdatedAt { conversation.state_updated_at = stateUpdatedAt }
                if let mode {
                    conversation.presentation_mode = mode
                    conversation.requires_action = mode == "needs_action"
                }
                self.conversation = conversation
                persistSnapshot(authoritative: true)
                onConversationUpdate?(conversation)
                emitAggregateTopologyInvalidationIfNeeded(
                    previousConversation: previousConversation,
                    previousState: previousState,
                    conversation: conversation)
            }
            clearResolvedActionIfStateAdvanced(
                currentState: ConversationState.parse(state))
            if let mode {
                // The server's presentation_mode (idle | working |
                // needs_action | error | done) is authoritative and covers
                // state variants this client predates.
                agentWorking = mode == "working"
            } else {
                // Fallback: states where the agent is waiting on the user
                // (or finished), mirroring ConversationState in ui/src/api.ts.
                let type = state.stringValue ?? state["type"]?.stringValue
                let restingStates: Set<String> = [
                    "idle", "error", "terminal", "context_exhausted", "handed_off",
                    "awaiting_user_response", "awaiting_task_approval",
                    "awaiting_recovery",
                ]
                agentWorking = type.map { !restingStates.contains($0) } ?? false
            }

        case .token(let seq, let text, let requestId):
            guard applyIfNewer(seq) else { return }
            // A late/replayed token after the turn closed would recreate a
            // ghost bubble below the finalized message — only accumulate
            // while a turn is actually running.
            guard agentWorking else { return }
            if streamingRequestId != requestId {
                streamingRequestId = requestId
                streamingText = ""
            }
            streamingText += text

        case .agentDone(let seq):
            guard applyIfNewer(seq) else { return }
            // agent_done follows the turn's final commit, so all transcript
            // rows observed before this boundary are durable.
            durableMessageSequenceCeiling = max(durableMessageSequenceCeiling, seq)
            streamingText = ""
            streamingRequestId = nil
            agentWorking = false
            // agent_done can close a turn without a trailing idle
            // state_change; leave resting/needs-action states alone but
            // clear in-flight ones so the spinner doesn't outlive the turn.
            let shouldMoveToIdle = Self.shouldMoveToIdleOnAgentDone(
                presentationMode: presentationMode,
                typedState: typedState,
                cancelledCommissionApproval: cancelNeedsAgentDoneFallback)
            cancelNeedsAgentDoneFallback = false
            if case .cancel = actionAttempt?.action {
                actionAttempt = nil
            }
            if shouldMoveToIdle {
                let previousConversation = conversation
                let previousState = typedState
                conversation?.state = .string("idle")
                // The mode must move with the state, or the snapshot
                // persists idle-with-working-mode and a cold reopen seeds
                // the spinner back for a turn that already ended.
                presentationMode = "idle"
                conversation?.presentation_mode = "idle"
                conversation?.requires_action = false
                if let conversation {
                    onConversationUpdate?(conversation)
                    emitAggregateTopologyInvalidationIfNeeded(
                        previousConversation: previousConversation,
                        previousState: previousState,
                        conversation: conversation)
                }
            }
            // Turn boundary: steering-queued entries should now be in
            // history; also a natural moment to send anything pending.
            // Snapshot first — same ordering rule as the message branch.
            persistSnapshot(
                authoritative: true,
                reconcileOutboxOnSuccess: true,
                drainOutboxAfter: true)

        case .conversationUpdate(let seq, let update):
            guard applyIfNewer(seq) else { return }
            // Shallow-merge the partial metadata payload (cwd, branch,
            // title, mode label after e.g. task approval) onto the local
            // conversation — this event exists precisely so clients don't
            // need a reconnect to see it.
            if var conv = conversation {
                let previousConversation = conversation
                let previousState = typedState
                if let v = update["cwd"]?.stringValue { conv.cwd = v }
                if let v = update["branch_name"]?.stringValue { conv.branch_name = v }
                if let v = update["task_title"]?.stringValue { conv.task_title = v }
                if let v = update["title"]?.stringValue { conv.title = v }
                if let v = update["conv_mode_label"]?.stringValue { conv.conv_mode_label = v }
                if let v = update["slug"]?.stringValue { conv.slug = v }
                if let v = update["title"]?.stringValue { conv.title = v }
                if let v = update["updated_at"]?.stringValue { conv.updated_at = v }
                conversation = conv
                persistSnapshot(authoritative: true)
                onConversationUpdate?(conv)
                emitAggregateTopologyInvalidationIfNeeded(
                    previousConversation: previousConversation,
                    previousState: previousState,
                    conversation: conv)
            }

        case .steerMessageQueued(let seq, let messageId):
            guard applyIfNewer(seq) else { return }
            outbox.markAccepted(messageId, steering: true)

        case .errorEvent(let seq, let message, let retryable):
            guard applyIfNewer(seq) else { return }
            lastErrorToast = message
            onSessionEvent?(.errorToastChanged(lastErrorToast))
            if retryable, actionAttempt?.action.waitsForAuthoritativeStateChange == true {
                actionAttempt = nil
            }

        case .conversationBecameTerminal(let seq):
            _ = applyIfNewer(seq)

        case .conversationHardDeleted(let seq, let deletedConversationId):
            guard deletedConversationId == conversationId, applyIfNewer(seq) else { return }
            handleHardDeletion()

        case .other(_, let seq):
            if let seq { _ = applyIfNewer(seq) }
        }
    }

    private func handleHardDeletion() {
        guard !isHardDeleted, !isHardDeletePending else { return }
        invalidateOutboxAuthority()
        invalidateLiveWork()
        streamTask?.cancel()
        streamTask = nil
        drainTask?.cancel()
        drainTask = nil
        staleCheckTask?.cancel()
        staleCheckTask = nil
        let hardDeleteContext = HardDeleteContext(
            conversationId: conversationId,
            aggregateAuthority: conversation?.aggregateIdentity ?? aggregateAuthority,
            configurationIdentity: api.configurationIdentity)
        isHardDeletePending = true
        actionAttempt = nil
        connection = .idle
        hardDeleteReportTask = Task { @MainActor [hardDeleteContext, onHardDeleted] in
            await onHardDeleted(hardDeleteContext)
        }
    }

    private func applyIfNewer(_ seq: Int64) -> Bool {
        guard seq > lastSequenceId else { return false }
        lastSequenceId = seq
        return true
    }

    nonisolated static func mergeDisplayData(existing: JSONValue?, patch: JSONValue) -> JSONValue {
        guard case .object(var merged) = existing,
              case .object(let patchObject) = patch
        else { return patch }

        for (key, value) in patchObject {
            if key == "tool_starts",
               case .object(var starts) = merged[key],
               case .object(let newStarts) = value {
                starts.merge(newStarts) { _, latest in latest }
                merged[key] = .object(starts)
            } else {
                merged[key] = value
            }
        }
        return .object(merged)
    }

    nonisolated static func shouldMoveToIdleOnAgentDone(
        presentationMode: String?,
        typedState: ConversationState,
        cancelledCommissionApproval: Bool
    ) -> Bool {
        cancelledCommissionApproval
            || presentationMode == "working"
            || (presentationMode == nil && typedState.isKnownWorkingState)
    }

    private func clearResolvedActionIfStateAdvanced(currentState: ConversationState) {
        guard let attempt = actionAttempt,
              attempt.action.waitsForAuthoritativeStateChange
        else {
            return
        }
        guard Self.actionStillAwaitsOriginalState(
            action: attempt.action, origin: attempt.originState, current: currentState)
        else {
            actionAttempt = nil
            return
        }
    }

    nonisolated static func actionStillAwaitsOriginalState(
        action: ConversationAction,
        origin: ConversationState?,
        current: ConversationState
    ) -> Bool {
        switch action {
        case .cancel:
            return current.isCancellable
        case .dismissError, .approveTask, .rejectTask, .provideTaskFeedback,
             .respondToQuestions, .dismissQuestion:
            return current == origin
        }
    }

    private func upsert(_ message: Message) {
        var message = message
        if let patch = pendingMessagePatches.removeValue(forKey: message.message_id) {
            if let content = patch.content { message.content = content }
            if let displayData = patch.displayData {
                message.display_data = Self.mergeDisplayData(
                    existing: message.display_data, patch: displayData)
            }
            if let durationMs = patch.durationMs {
                message.display_data = Self.mergeDisplayData(
                    existing: message.display_data,
                    patch: .object(["duration_ms": .number(durationMs)]))
            }
        }
        if let idx = messages.firstIndex(where: { $0.message_id == message.message_id }) {
            // Eager (in-flight) messages are later re-broadcast persisted
            // with the same message_id; the second arrival refreshes fields.
            messages[idx] = message
        } else {
            messages.append(message)
            messages.sort { $0.sequence_id < $1.sequence_id }
        }
    }

    private func reconcileAlreadyPersisted(_ localId: String) async {
        do {
            let response = try await api.reconcileAcceptedMessages(
                conversationId: conversationId, messageIds: [localId])
            guard !Task.isCancelled,
                  let result = response.entries.first(where: { $0.message_id == localId })
            else { return }
            switch result.status {
            case .persisted:
                guard let message = result.message else { return }
                upsert(message)
                durableMessageSequenceCeiling = max(
                    durableMessageSequenceCeiling, message.sequence_id)
                lastSequenceId = max(lastSequenceId, message.sequence_id)
                rebuildToolUseIndex()
                persistSnapshot(authoritative: true, reconcileOutboxOnSuccess: true)
            case .steeringQueued:
                outbox.markAccepted(localId, steering: true)
            case .absent:
                // The POST response and exact reconciliation disagree. Keep
                // the accepted outbox row visible and force the live stream
                // through a fresh authoritative init rather than guessing.
                restartStreamForResync()
            }
        } catch {
            if !Task.isCancelled {
                restartStreamForResync()
            }
        }
    }

    private func restartStreamForResync() {
        guard streamTask != nil, viewIsActive, !streamBlockedUntilConfigurationChange else { return }
        invalidateLiveWork()
        let generation = liveWorkGeneration
        let apiIdentity = api.configurationIdentity
        streamTask?.cancel()
        streamTask = Task { await streamLoop(generation: generation, apiIdentity: apiIdentity) }
    }

    nonisolated static func reconcileTranscript(
        existing: [Message],
        incoming: [Message],
        coverage: PhoenixEvent.InitSnapshot.TranscriptCoverage,
        generationMatches: Bool
    ) -> [Message] {
        guard generationMatches else {
            return incoming.sorted { $0.sequence_id < $1.sequence_id }
        }
        switch coverage {
        case .complete:
            return incoming.sorted { $0.sequence_id < $1.sequence_id }
        case .preserve:
            return existing.sorted { $0.sequence_id < $1.sequence_id }
        case .tail:
            var byId = Dictionary(uniqueKeysWithValues: existing.map { ($0.message_id, $0) })
            for message in incoming {
                byId[message.message_id] = message
            }
            return byId.values.sorted { $0.sequence_id < $1.sequence_id }
        }
    }

    nonisolated static func replayFloor(
        previous: Int64,
        anchor: Int64,
        serverTip: Int64,
        generationMatches: Bool,
        restoredFromDisk: Bool
    ) -> Int64 {
        if restoredFromDisk || previous == 0 || !generationMatches || serverTip < previous {
            return anchor
        }
        return max(previous, anchor)
    }

    nonisolated static func durableMessages(
        _ messages: [Message], through sequenceCeiling: Int64
    ) -> [Message] {
        messages.filter { $0.sequence_id <= sequenceCeiling }
    }

    nonisolated static func durableCeilingAfterInit(
        anchor: Int64, messages: [Message]
    ) -> Int64 {
        max(anchor, messages.map(\.sequence_id).max() ?? 0)
    }

    nonisolated static func durableCeilingAfterLiveMessage(
        current: Int64, message: Message
    ) -> Int64 {
        // The wire shares one event shape for eager and committed assistant
        // messages. Every other message type is emitted only after commit.
        guard message.message_type != "agent" else { return current }
        return max(current, message.sequence_id)
    }

    private func reconcileOutbox() {
        outbox.reconcile(
            authoritativeMessageIds: Set(
                Self.durableMessages(
                    messages, through: durableMessageSequenceCeiling
                ).map(\.message_id)))
    }

    private func emitAggregateTopologyInvalidationIfNeeded(
        previousConversation: Conversation?,
        previousState: ConversationState,
        conversation: Conversation
    ) {
        let currentState = typedState
        if previousConversation?.aggregateIdentity != conversation.aggregateIdentity,
           let previousAggregateIdentity = previousConversation?.aggregateIdentity {
            onSessionEvent?(
                .aggregateTopologyInvalidated(
                    ProductConversationTopologyInvalidation(
                        transcriptRowId: conversation.transcriptRowIdentity,
                        aggregateIdentity: conversation.aggregateIdentity,
                        reason: .aggregateIdentityChanged(
                            previous: previousAggregateIdentity,
                            current: conversation.aggregateIdentity))))
            return
        }
        guard previousState != currentState else { return }
        let reason: ProductConversationTopologyInvalidation.Reason
        if let topologyReason = currentState.productConversationTopologyInvalidationReason {
            reason = topologyReason
        } else if previousState.acceptsChatMessage != currentState.acceptsChatMessage {
            reason = .chatCapabilityChanged
        } else {
            return
        }
        onSessionEvent?(
            .aggregateTopologyInvalidated(
                ProductConversationTopologyInvalidation(
                    transcriptRowId: conversation.transcriptRowIdentity,
                    aggregateIdentity: conversation.aggregateIdentity,
                    reason: reason)))
    }

    private func rebuildToolUseIndex() {
        var index: [String: ToolUseRef] = [:]
        for message in messages where message.message_type == "agent" {
            guard let blocks = message.content.arrayValue else { continue }
            for block in blocks where block["type"]?.stringValue == "tool_use" {
                guard let id = block["id"]?.stringValue,
                      let name = block["name"]?.stringValue
                else { continue }
                index[id] = ToolUseRef(name: name, input: block["input"])
            }
        }
        toolUseIndex = index
        onSessionEvent?(.messagesChanged)
    }
}

/// The identity of a tool invocation, joined from an agent message's
/// tool_use block to the tool result that answers it.
struct ToolUseRef: Equatable {
    var name: String
    var input: JSONValue?
}
