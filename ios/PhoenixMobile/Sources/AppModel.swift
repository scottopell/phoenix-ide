import Foundation
import Observation
import UserNotifications

struct CachedProductHistory: Codable, Equatable, Sendable {
    var snapshot: ProductConversationSnapshot
    var fetchedAt: Date
}

@MainActor
enum ProductHistorySnapshotStore {
    static let schemaVersion = 2

    static func cacheName(productConversationId: String) -> String {
        "product-history-\(productConversationId)"
    }

    static func load(productConversationId: String) -> CachedProductHistory? {
        let cached = DiskStore.loadVersioned(
            CachedProductHistory.self,
            name: cacheName(productConversationId: productConversationId),
            version: schemaVersion)
        guard let cached, cached.snapshot.product_conversation_id == productConversationId else {
            return nil
        }
        return cached
    }

    static func writer(productConversationId: String) -> VersionedDiskWriter {
        DiskStore.versionedWriter(
            name: cacheName(productConversationId: productConversationId),
            version: schemaVersion)
    }

    nonisolated static func merging(
        _ accumulated: ProductConversationSnapshot?,
        page: ProductConversationSnapshot
    ) throws -> ProductConversationSnapshot {
        guard var merged = accumulated else {
            var first = page
            first.segments = try normalizedSegments(page.segments)
            first.before = nil
            first.has_older = false
            return first
        }
        guard page.product_conversation_id == merged.product_conversation_id else {
            throw ProductHistoryLoadError.aggregateIdentityChanged
        }

        var segmentsByOrdinal = Dictionary(uniqueKeysWithValues: merged.segments.map {
            ($0.segment_ordinal, $0)
        })
        for segment in page.segments {
            if var existing = segmentsByOrdinal[segment.segment_ordinal] {
                guard existing.transcript_row_id == segment.transcript_row_id else {
                    throw ProductHistoryLoadError.segmentIdentityChanged
                }
                var messagesById: [String: Message] = [:]
                for message in existing.messages + segment.messages
                    where messagesById[message.message_id] == nil
                {
                    messagesById[message.message_id] = message
                }
                existing.messages = messagesById.values.sorted {
                    ($0.sequence_id, $0.message_id) < ($1.sequence_id, $1.message_id)
                }
                if existing.handoff == nil { existing.handoff = segment.handoff }
                segmentsByOrdinal[segment.segment_ordinal] = existing
            } else {
                segmentsByOrdinal[segment.segment_ordinal] = normalizedSegment(segment)
            }
        }
        merged.segments = segmentsByOrdinal.values.sorted {
            ($0.segment_ordinal, $0.transcript_row_id) < ($1.segment_ordinal, $1.transcript_row_id)
        }
        merged.before = nil
        merged.has_older = false
        return merged
    }

    private nonisolated static func normalizedSegments(
        _ segments: [ProductConversationSegment]
    ) throws -> [ProductConversationSegment] {
        var byOrdinal: [Int64: ProductConversationSegment] = [:]
        for segment in segments {
            if let existing = byOrdinal[segment.segment_ordinal],
               existing.transcript_row_id != segment.transcript_row_id
            {
                throw ProductHistoryLoadError.segmentIdentityChanged
            }
            byOrdinal[segment.segment_ordinal] = normalizedSegment(segment)
        }
        return byOrdinal.values.sorted {
            ($0.segment_ordinal, $0.transcript_row_id) < ($1.segment_ordinal, $1.transcript_row_id)
        }
    }

    private nonisolated static func normalizedSegment(
        _ segment: ProductConversationSegment
    ) -> ProductConversationSegment {
        var normalized = segment
        var messagesById: [String: Message] = [:]
        for message in segment.messages where messagesById[message.message_id] == nil {
            messagesById[message.message_id] = message
        }
        normalized.messages = messagesById.values.sorted {
            ($0.sequence_id, $0.message_id) < ($1.sequence_id, $1.message_id)
        }
        return normalized
    }
}

enum ProductCloseConfirmationKind: Equatable {
    case stopWork
    case losses
    case repair
}

struct ProductCloseLossInventory {
    static func isComplete(_ losses: [ProductConversationCloseLoss]) -> Bool {
        !losses.isEmpty && losses.allSatisfy {
            !$0.scope.isEmpty && !$0.category.isEmpty && !$0.identity.isEmpty
        }
    }

    static func message(_ losses: [ProductConversationCloseLoss]) -> String {
        losses
            .sorted {
                ($0.scope, $0.category, $0.identity) < ($1.scope, $1.category, $1.identity)
            }
            .map { "Scope: \($0.scope)\nCategory: \($0.category)\nItem: \($0.identity)" }
            .joined(separator: "\n\n")
    }
}

struct PendingProductCloseConfirmation: Equatable {
    static func isCompleted(snapshot: ProductConversationSnapshot) -> Bool {
        snapshot.close?.phase == .completed || snapshot.ordinary_lifecycle == .history
    }

    var productConversationId: String
    var transcriptRowId: String
    var close: ProductConversationClose

    init?(snapshot: ProductConversationSnapshot) {
        guard let close = snapshot.close,
              close.phase == .awaiting_stop_work_confirmation
                || close.phase == .awaiting_loss_confirmation
                || close.phase == .needs_repair
        else { return nil }
        productConversationId = snapshot.product_conversation_id
        transcriptRowId = snapshot.latest_transcript_row_id
        self.close = close
    }

    init(productConversationId: String, transcriptRowId: String, close: ProductConversationClose) {
        self.productConversationId = productConversationId
        self.transcriptRowId = transcriptRowId
        self.close = close
    }

    var kind: ProductCloseConfirmationKind? {
        switch close.phase {
        case .awaiting_stop_work_confirmation: .stopWork
        case .awaiting_loss_confirmation: .losses
        case .needs_repair: .repair
        default: nil
        }
    }
}

enum ProductHistoryLoadError: Error, LocalizedError, Equatable {
    case emptyResponse
    case aggregateIdentityChanged
    case segmentIdentityChanged
    case missingCursor
    case repeatedCursor
    case staleServerGeneration
    case notFound

    var errorDescription: String? {
        switch self {
        case .emptyResponse: "The server returned no Product History snapshot."
        case .aggregateIdentityChanged: "Product History changed identity while loading."
        case .segmentIdentityChanged: "Product History lineage changed while loading."
        case .missingCursor, .repeatedCursor: "The server returned an invalid Product History page cursor."
        case .staleServerGeneration: "Product History was invalidated while loading."
        case .notFound: "This conversation was deleted or is no longer available."
        }
    }
}

struct ProductActionGenerationTracker {
    private var generations: [String: Int] = [:]

    mutating func begin(productConversationId: String) -> Int {
        let generation = (generations[productConversationId] ?? 0) + 1
        generations[productConversationId] = generation
        return generation
    }

    func isCurrent(_ generation: Int, productConversationId: String) -> Bool {
        generations[productConversationId] == generation
    }

    mutating func reset() {
        generations.removeAll()
    }

    mutating func end(_ generation: Int, productConversationId: String) {
        if isCurrent(generation, productConversationId: productConversationId) {
            generations[productConversationId] = nil
        }
    }
}

struct ProductCloseResolutionTracker {
    private var nextGeneration = 0
    private var current: (productConversationId: String, generation: Int)?

    var isInFlight: Bool { current != nil }

    mutating func begin(productConversationId: String) -> Int? {
        guard current == nil else { return nil }
        nextGeneration &+= 1
        current = (productConversationId, nextGeneration)
        return nextGeneration
    }

    func isCurrent(_ generation: Int, productConversationId: String) -> Bool {
        current?.generation == generation
            && current?.productConversationId == productConversationId
    }

    mutating func end(_ generation: Int, productConversationId: String) {
        guard isCurrent(generation, productConversationId: productConversationId) else { return }
        current = nil
    }

    mutating func reset() {
        current = nil
    }
}

/// Root composition: server settings, connectivity, API client, stores, and
/// the active per-conversation sessions.
@MainActor
@Observable
final class AppModel {
    // MARK: - Settings

    private static let serverURLKey = "phoenix.serverURL"
    private static let trustSelfSignedKey = "phoenix.trustSelfSigned"
    private static let passwordAccount = "server-password"
    /// Shared with NewConversationView's @AppStorage. Cleared on sign-out:
    /// the value is a server-local filesystem path and must not leak (or be
    /// sent) to a different server configured later.
    static let lastCwdKey = "phoenix.lastCwd"
    private static let coordinatorIdKey = "phoenix.coordinatorConversationId"

    var serverURLString: String {
        didSet {
            UserDefaults.standard.set(serverURLString, forKey: Self.serverURLKey)
            rebuildAPI()
        }
    }

    private(set) var password: String

    var trustSelfSigned: Bool {
        didSet {
            UserDefaults.standard.set(trustSelfSigned, forKey: Self.trustSelfSignedKey)
            rebuildAPI()
        }
    }

    var isConfigured: Bool {
        api != nil
    }

    // MARK: - Services

    let connectivity = ConnectivityMonitor()
    let listStore = ConversationListStore()
    private(set) var api: PhoenixAPI?
    /// Invalidates responses started with earlier server credentials or URL.
    private var apiGeneration = 0
    private var aggregateEventTask: Task<Void, Never>?
    private var isForeground = true

    /// Sessions for conversations the user has opened, kept alive so their
    /// outboxes continue draining while the user navigates elsewhere.
    private var sessions: [String: ConversationSession] = [:]
    /// Short-lived delivery owners for persisted outboxes whose conversation
    /// is not open. Retaining one per conversation serializes every trigger
    /// through the session's single drain task.
    private var drainSessions: [String: ConversationSession] = [:]
    private var closingProductConversationIds: Set<String> = []
    private var closeActionGenerations = ProductActionGenerationTracker()
    private var productHistoryGenerations = ProductActionGenerationTracker()
    private var confirmationRehydrationGenerations = ProductActionGenerationTracker()
    private(set) var deletedProductHistoryIds: Set<String> = []
    private(set) var pendingProductCloseConfirmation: PendingProductCloseConfirmation?
    private var pendingProductCloseResolution = ProductCloseResolutionTracker()
    var isResolvingPendingProductClose: Bool {
        pendingProductCloseResolution.isInFlight
    }

    init() {
        serverURLString = UserDefaults.standard.string(forKey: Self.serverURLKey) ?? ""
        password = Keychain.password(account: Self.passwordAccount) ?? ""
        trustSelfSigned = UserDefaults.standard.object(forKey: Self.trustSelfSignedKey) as? Bool ?? true
        attention = AttentionMonitor(
            currentConversations: listStore.conversations,
            transcriptToAggregate: listStore.transcriptToAggregate)
        rebuildAPI()
        _ = connectivity.addRestoreObserver { [weak self] in
            Task { await self?.reconcileListThenResumeAndDrain() }
        }
        notificationRouter.model = self
        UNUserNotificationCenter.current().delegate = notificationRouter
    }

    private func rebuildAPI() {
        apiGeneration += 1
        aggregateEventTask?.cancel()
        aggregateEventTask = nil
        guard let url = URL(string: serverURLString), url.host != nil else {
            api = nil
            return
        }
        let rebuiltAPI = PhoenixAPI(
            baseURL: url,
            password: password.isEmpty ? nil : password,
            allowSelfSigned: trustSelfSigned)
        api = rebuiltAPI
        guard let rebuiltAPI else { return }
        for session in sessions.values { session.replaceAPI(rebuiltAPI) }
        for session in drainSessions.values { session.replaceAPI(rebuiltAPI) }
        if isForeground { startAggregateEventStream(api: rebuiltAPI, generation: apiGeneration) }
    }

    func configure(serverURL: String, password: String, trustSelfSigned: Bool) throws {
        try Keychain.setPassword(password, account: Self.passwordAccount)
        self.password = password
        self.trustSelfSigned = trustSelfSigned
        serverURLString = serverURL
    }

    func session(for conversationId: String) -> ConversationSession? {
        guard let api else { return nil }
        if let existing = sessions[conversationId] { return existing }
        let onConversationUpdate: (Conversation) -> Void = { [weak self] conversation in
            self?.handleSessionConversationUpdate(conversation, transcriptRowId: conversationId)
        }
        let onHardDeleted: (String) -> Void = { [weak self] deletedId in
            self?.handleHardDeleted(deletedId, aggregateIdentity: self?.aggregateIdentity(forTranscriptRowId: conversationId))
        }
        let session: ConversationSession
        if let draining = drainSessions.removeValue(forKey: conversationId) {
            draining.adoptOpenOwnership(
                onConversationUpdate: onConversationUpdate,
                onHardDeleted: onHardDeleted)
            session = draining
        } else {
            session = ConversationSession(
                conversationId: conversationId, api: api, connectivity: connectivity,
                onConversationUpdate: onConversationUpdate,
                onHardDeleted: onHardDeleted)
        }
        sessions[conversationId] = session
        return session
    }

    private func aggregateIdentity(forTranscriptRowId transcriptRowId: String) -> String? {
        listStore.aggregateId(forTranscriptRowId: transcriptRowId)
    }

    private func mergeAggregateProjection(
        existing: Conversation,
        liveUpdate: Conversation,
        aggregateIdentity: String
    ) -> Conversation {
        Conversation(
            id: existing.id == liveUpdate.id ? liveUpdate.id : existing.id,
            product_conversation_id: aggregateIdentity,
            chain_root_id: existing.chain_root_id,
            slug: existing.slug,
            title: existing.title,
            model: liveUpdate.model,
            cwd: liveUpdate.cwd,
            created_at: liveUpdate.created_at,
            updated_at: liveUpdate.updated_at,
            message_count: liveUpdate.message_count,
            state: liveUpdate.state,
            state_updated_at: liveUpdate.state_updated_at,
            branch_name: liveUpdate.branch_name,
            task_title: existing.task_title,
            archived: existing.archived,
            product_close_action: existing.product_close_action,
            project_name: liveUpdate.project_name,
            conv_mode_label: liveUpdate.conv_mode_label,
            presentation_mode: liveUpdate.presentation_mode,
            requires_action: liveUpdate.requires_action,
            transcript_generation: liveUpdate.transcript_generation,
            runtime_role: existing.runtime_role ?? liveUpdate.runtime_role)
    }

    private func handleSessionConversationUpdate(_ conversation: Conversation, transcriptRowId: String) {
        guard let aggregateIdentity = aggregateIdentity(forTranscriptRowId: transcriptRowId),
              let existing = listStore.conversations.first(where: { $0.aggregateIdentity == aggregateIdentity })
        else {
            listStore.upsert(conversation)
            return
        }
        listStore.upsert(
            mergeAggregateProjection(
                existing: existing,
                liveUpdate: conversation,
                aggregateIdentity: aggregateIdentity))
    }

    private func handleHardDeleted(_ conversationId: String, aggregateIdentity: String?) {
        let notificationId: String
        if let aggregateIdentity {
            listStore.remove(aggregateId: aggregateIdentity)
            notificationId = aggregateIdentity
        } else {
            listStore.removeByTranscriptRowId(conversationId)
            notificationId = conversationId
        }
        if pendingOpenConversationId == conversationId {
            pendingOpenConversationId = nil
        }
        UNUserNotificationCenter.current().removeDeliveredNotifications(
            withIdentifiers: ["attention-\(notificationId)"])
        UNUserNotificationCenter.current().removePendingNotificationRequests(
            withIdentifiers: ["attention-\(notificationId)"])
    }

    private func startAggregateEventStream(api: PhoenixAPI, generation: Int) {
        guard aggregateEventTask == nil else { return }
        aggregateEventTask = Task { [weak self] in
            var retryDelay = 1.0
            while !Task.isCancelled {
                guard let self, self.apiGeneration == generation, self.isForeground else { return }
                if !self.connectivity.isOnline {
                    try? await Task.sleep(for: .seconds(30))
                    continue
                }
                do {
                    let bytes = try await api.openProductConversationEventStream()
                    retryDelay = 1
                    var parser = SSEParser()
                    for try await byte in bytes {
                        if Task.isCancelled { return }
                        if let frame = parser.consume(byte),
                           let deletion = ProductConversationDeletionEvent.decode(frame: frame)
                        {
                            await self.handleAggregateHardDeleted(deletion, generation: generation)
                        }
                    }
                } catch let error as APIError where error.isPermanentStreamAuthenticationFailure {
                    return
                } catch is CancellationError {
                    return
                } catch {
                    if Task.isCancelled { return }
                }
                let jitter = Double.random(in: 0...0.3) * retryDelay
                try? await Task.sleep(for: .seconds(retryDelay + jitter))
                retryDelay = min(retryDelay * 2, 30)
            }
        }
    }

    private func handleAggregateHardDeleted(
        _ deletion: ProductConversationDeletionEvent,
        generation: Int
    ) async {
        guard apiGeneration == generation else { return }
        let aggregateId = deletion.conversation_id
        let cachedIds = cachedProductHistory(productConversationId: aggregateId)?
            .snapshot.segments.map(\.transcript_row_id) ?? []
        let transcriptIds = Set(
            deletion.deleted_conversation_ids
                + listStore.transcriptRowIds(forAggregateId: aggregateId)
                + cachedIds)
        _ = await removeProductHistoryLocally(
            productConversationId: aggregateId,
            transcriptIds: transcriptIds,
            startedGeneration: generation)
    }

    func refreshList() async {
        guard let api else { return }
        attentionEvidenceGeneration &+= 1
        await listStore.refresh(api: api)
        if listStore.lastError == nil {
            await rehydratePendingProductCloseConfirmation(api: api)
            // The user is looking at fresh data — nothing here should nudge
            // them later.
            attention.seed(
                with: listStore.conversations,
                transcriptToAggregate: listStore.transcriptToAggregate)
        }
    }

    private func rehydratePendingProductCloseConfirmation(api: PhoenixAPI) async {
        guard pendingProductCloseConfirmation == nil else { return }
        let fenceIdentity = "pending-close-confirmation"
        let rehydrationGeneration = confirmationRehydrationGenerations.begin(
            productConversationId: fenceIdentity)
        let startedGeneration = apiGeneration
        let activeCloseRows = listStore.conversations.filter {
            $0.product_close_action == .unavailable(reason: .active_close_attempt)
        }
        for row in activeCloseRows {
            guard let snapshot = try? await api.getProductConversation(reference: row.aggregateIdentity),
                  apiGeneration == startedGeneration,
                  pendingProductCloseConfirmation == nil,
                  confirmationRehydrationGenerations.isCurrent(
                    rehydrationGeneration, productConversationId: fenceIdentity)
            else { continue }
            if let pending = PendingProductCloseConfirmation(snapshot: snapshot) {
                pendingProductCloseConfirmation = pending
                return
            }
        }
    }

    // MARK: - Needs-attention nudges

    let attention: AttentionMonitor
    private let notificationRouter = NotificationRouter()
    private static let nudgesEnabledKey = "phoenix.backgroundNudges"
    private var nudgePreferenceGeneration = 0
    private var attentionEvidenceGeneration = 0

    private(set) var backgroundNudgesEnabled =
        UserDefaults.standard.bool(forKey: AppModel.nudgesEnabledKey)
    private(set) var nudgeAuthorizationHint: String?
    /// Set by a notification tap; the list view navigates and clears it.
    var pendingOpenConversationId: String?

    func resolvedNavigationConversationId(
        aggregateId: String?,
        latestTranscriptRowId: String
    ) -> String {
        if connectivity.isOnline {
            return latestTranscriptRowId
        }
        guard let aggregateId else { return latestTranscriptRowId }
        return listStore.cachedNavigationTranscriptRowId(
            forAggregateId: aggregateId,
            latestTranscriptRowId: latestTranscriptRowId)
    }

    func cachedProductHistory(productConversationId: String) -> CachedProductHistory? {
        ProductHistorySnapshotStore.load(productConversationId: productConversationId)
    }

    func notificationNavigationId(for notifiedId: String) -> String {
        guard let aggregateId = listStore.aggregateId(forTranscriptRowId: notifiedId) else {
            return notifiedId
        }
        if listStore.conversations.contains(where: {
            $0.aggregateIdentity == aggregateId && $0.archived == true
        }) {
            return aggregateId
        }
        return resolvedNavigationConversationId(
            aggregateId: aggregateId,
            latestTranscriptRowId: notifiedId)
    }

    func loadProductHistory(productConversationId: String) async throws -> CachedProductHistory {
        guard let api, connectivity.isOnline else {
            if let cached = cachedProductHistory(productConversationId: productConversationId) {
                return cached
            }
            throw APIError.transport(underlying: URLError(.notConnectedToInternet))
        }
        let startedGeneration = apiGeneration
        let writer = ProductHistorySnapshotStore.writer(productConversationId: productConversationId)
        let startedHistoryGeneration = productHistoryGenerations.begin(
            productConversationId: productConversationId)
        let revision = writer.reserveRevision()
        var snapshot: ProductConversationSnapshot?
        var before: String?
        var seenCursors: Set<String> = []
        do {
            repeat {
                let page = try await api.getProductConversation(
                    reference: productConversationId,
                    before: before)
                guard !Task.isCancelled,
                      apiGeneration == startedGeneration,
                      productHistoryGenerations.isCurrent(
                          startedHistoryGeneration,
                          productConversationId: productConversationId)
                else {
                    throw ProductHistoryLoadError.staleServerGeneration
                }
                guard page.product_conversation_id == productConversationId else {
                    throw ProductHistoryLoadError.aggregateIdentityChanged
                }
                snapshot = try await Task.detached(priority: .userInitiated) {
                    try ProductHistorySnapshotStore.merging(snapshot, page: page)
                }.value
                guard page.has_older else { break }
                guard let next = page.before, !next.isEmpty else {
                    throw ProductHistoryLoadError.missingCursor
                }
                guard seenCursors.insert(next).inserted else {
                    throw ProductHistoryLoadError.repeatedCursor
                }
                before = next
            } while true
        } catch let error as APIError where error.isNotFound {
            guard apiGeneration == startedGeneration else {
                throw ProductHistoryLoadError.staleServerGeneration
            }
            let cachedTranscriptIds = cachedProductHistory(productConversationId: productConversationId)?
                .snapshot.segments.map(\.transcript_row_id) ?? []
            let transcriptIds = Set(
                listStore.transcriptRowIds(forAggregateId: productConversationId)
                    + cachedTranscriptIds)
            guard await removeProductHistoryLocally(
                productConversationId: productConversationId,
                transcriptIds: transcriptIds,
                startedGeneration: startedGeneration)
            else {
                throw ProductHistoryLoadError.staleServerGeneration
            }
            throw ProductHistoryLoadError.notFound
        }

        guard !Task.isCancelled,
              apiGeneration == startedGeneration,
              productHistoryGenerations.isCurrent(
                  startedHistoryGeneration,
                  productConversationId: productConversationId),
              let snapshot
        else {
            throw ProductHistoryLoadError.staleServerGeneration
        }
        let cached = CachedProductHistory(snapshot: snapshot, fetchedAt: Date())
        guard await writer.save(cached, revision: revision),
              !Task.isCancelled,
              apiGeneration == startedGeneration,
              productHistoryGenerations.isCurrent(
                  startedHistoryGeneration,
                  productConversationId: productConversationId)
        else {
            throw ProductHistoryLoadError.staleServerGeneration
        }
        deletedProductHistoryIds.remove(productConversationId)
        return cached
    }

    func navigationConversationId(for conversation: Conversation) -> String {
        resolvedNavigationConversationId(
            aggregateId: conversation.product_conversation_id,
            latestTranscriptRowId: conversation.transcriptRowIdentity)
    }

    func setBackgroundNudges(_ enabled: Bool) async {
        nudgePreferenceGeneration &+= 1
        let generation = nudgePreferenceGeneration
        nudgeAuthorizationHint = nil
        if enabled {
            guard await AttentionMonitor.requestAuthorization() else {
                guard generation == nudgePreferenceGeneration else { return }
                nudgeAuthorizationHint =
                    "Notifications are off for Phoenix in iOS Settings — enable them there first."
                backgroundNudgesEnabled = false
                UserDefaults.standard.set(false, forKey: Self.nudgesEnabledKey)
                return
            }
        }
        guard generation == nudgePreferenceGeneration else { return }
        backgroundNudgesEnabled = enabled
        UserDefaults.standard.set(enabled, forKey: Self.nudgesEnabledKey)
        if enabled {
            BackgroundRefresh.scheduleNext()
        } else {
            BackgroundRefresh.cancelPending()
        }
    }

    /// One background-fetch cycle: fetch the list, notify on attention
    /// transitions, and opportunistically freshen the cached list so the
    /// next cold open is newer. Returns success for BGTask accounting.
    func runBackgroundAttentionCheck() async -> Bool {
        guard backgroundNudgesEnabled, let api else { return false }
        let startedGeneration = apiGeneration
        let startedNudgeGeneration = nudgePreferenceGeneration
        let startedEvidenceGeneration = attentionEvidenceGeneration
        let listToken = listStore.externalRefreshToken()
        guard let fresh = try? await api.listConversations() else { return false }
        let coordinator = await Self.coordinatorAttentionEvidence(
            rememberedId: coordinatorConversationId,
            fetch: { _ in try await api.getCoordinatorProjection() },
            cached: { ConversationSession.cachedConversation(conversationId: $0) })
        guard !Task.isCancelled,
              backgroundNudgesEnabled,
              apiGeneration == startedGeneration,
              nudgePreferenceGeneration == startedNudgeGeneration,
              attentionEvidenceGeneration == startedEvidenceGeneration,
              listStore.canApplyExternal(startedAt: listToken)
        else { return false }
        guard listStore.applyExternal(fresh, startedAt: listToken) else { return false }
        let attentionConversations = Self.attentionConversations(
            ordinary: fresh,
            coordinator: coordinator)
        let isCurrent: @MainActor () -> Bool = { [weak self] in
            guard let self else { return false }
            return self.backgroundNudgesEnabled
                && self.apiGeneration == startedGeneration
                && self.nudgePreferenceGeneration == startedNudgeGeneration
                && self.attentionEvidenceGeneration == startedEvidenceGeneration
        }
        await attention.refreshAndNotifyIfNeeded(
            from: attentionConversations,
            transcriptToAggregate: listStore.transcriptToAggregate,
            isCurrent: isCurrent)
        return await isCurrent()
    }

    static func coordinatorAttentionEvidence(
        rememberedId: String?,
        fetch: (String) async throws -> Conversation,
        cached: (String) -> Conversation?
    ) async -> Conversation? {
        try? await coordinatorForAttention(
            rememberedId: rememberedId,
            fetch: fetch,
            cached: cached)
    }

    static func coordinatorForAttention(
        rememberedId: String?,
        fetch: (String) async throws -> Conversation,
        cached: (String) -> Conversation?
    ) async throws -> Conversation? {
        guard let rememberedId else { return nil }
        do {
            return try await fetch(rememberedId)
        } catch let error as APIError where error.isTransport {
            guard let cached = cached(rememberedId) else { throw error }
            return cached
        }
    }

    nonisolated static func attentionConversations(
        ordinary: [Conversation],
        coordinator: Conversation?
    ) -> [Conversation] {
        guard let coordinator else { return ordinary }
        return ordinary.filter { $0.aggregateIdentity != coordinator.aggregateIdentity } + [coordinator]
    }

    // MARK: - Coordinator

    /// The fleet Coordinator's conversation id, remembered across launches
    /// so its cached transcript opens offline and its list row is badged.
    /// Per-server state — cleared on sign-out.
    private(set) var coordinatorConversationId: String? =
        UserDefaults.standard.string(forKey: AppModel.coordinatorIdKey)

    var coordinatorAvailableOffline: Bool {
        guard let id = coordinatorConversationId else { return false }
        return ConversationSession.hasCachedSnapshot(conversationId: id)
    }


    /// Resolve the Coordinator conversation to open. Online: get-or-create
    /// on the server (it's an ordinary conversation; everything downstream
    /// is the normal conversation surface). Offline: fall back to the
    /// remembered id so the cached transcript still opens — asking new
    /// questions then queues through the outbox like any conversation.
    func openCoordinator() async -> String? {
        if let api, connectivity.isOnline {
            let startedGeneration = apiGeneration
            do {
                let conversation = try await api.ensureCoordinator()
                guard apiGeneration == startedGeneration, connectivity.isOnline else {
                    return nil
                }
                coordinatorConversationId = conversation.id
                UserDefaults.standard.set(conversation.id, forKey: Self.coordinatorIdKey)
                listStore.upsert(conversation)
                return conversation.id
            } catch {
                guard !Task.isCancelled, apiGeneration == startedGeneration else { return nil }
                if let apiError = error as? APIError,
                   apiError.isTransport,
                   let cached = coordinatorConversationId,
                   ConversationSession.hasCachedSnapshot(conversationId: cached) {
                    return cached
                }
                lastActionError = (error as? APIError)?.errorDescription
                    ?? error.localizedDescription
                return nil
            }
        }
        if let cached = coordinatorConversationId,
           ConversationSession.hasCachedSnapshot(conversationId: cached) {
            return cached
        }
        lastActionError = "Opening the Coordinator offline needs a cached conversation."
        return nil
    }

    /// Online-only archive. Returns false with `lastActionError` on failure.
    var lastActionError: String?

    private func requireEmptyAggregateOutboxes(transcriptIds: Set<String>) async -> Bool {
        let hasVisibleMessages = transcriptIds.contains {
            sessions[$0]?.outbox.visibleEntries.isEmpty == false
        }
        guard !hasVisibleMessages else {
            lastActionError = "This conversation has queued or unconfirmed messages. Retry or discard them before closing."
            return false
        }
        for transcriptId in transcriptIds {
            if let session = sessions[transcriptId] {
                _ = await session.outbox.flushPersistence()
            }
        }
        guard transcriptIds.allSatisfy({
            if case .empty = Outbox.storedContents(conversationId: $0) { return true }
            return false
        }) else {
            lastActionError = "This conversation has queued or unreadable messages. Resolve them before closing."
            return false
        }
        return true
    }

    @discardableResult
    func closeProductConversation(_ conversation: Conversation) async -> Bool {
        _ = confirmationRehydrationGenerations.begin(
            productConversationId: "pending-close-confirmation")
        guard ClientOperation.close.policy == .onlineOnly else { return false }
        let conversationId = conversation.transcriptRowIdentity
        let transcriptIds = Set(
            listStore.transcriptRowIds(forAggregateId: conversation.aggregateIdentity)
                + [conversationId])
        let startedGeneration = apiGeneration
        guard let api, connectivity.isOnline else {
            lastActionError = "Closing needs a connection — it can't be queued."
            return false
        }
        guard closingProductConversationIds.insert(conversation.aggregateIdentity).inserted else {
            return false
        }
        let startedCloseActionGeneration = closeActionGenerations.begin(
            productConversationId: conversation.aggregateIdentity)
        defer {
            closingProductConversationIds.remove(conversation.aggregateIdentity)
            closeActionGenerations.end(
                startedCloseActionGeneration,
                productConversationId: conversation.aggregateIdentity)
        }
        guard await requireEmptyAggregateOutboxes(transcriptIds: transcriptIds) else {
            return false
        }
        guard apiGeneration == startedGeneration else { return false }
        let aggregateSessions = transcriptIds.compactMap { transcriptId in
            session(for: transcriptId).map { (transcriptId, $0) }
        }
        var fencedSessions: [ConversationSession] = []
        for (_, session) in aggregateSessions {
            guard session.beginArchiving() else {
                fencedSessions.forEach { $0.endArchiving() }
                lastActionError = "This conversation has queued or unconfirmed messages. Retry or discard them before closing."
                return false
            }
            fencedSessions.append(session)
        }
        var closed = false
        defer { if !closed { fencedSessions.forEach { $0.endArchiving() } } }
        do {
            _ = try await loadProductHistory(productConversationId: conversation.aggregateIdentity)
            guard apiGeneration == startedGeneration else { return false }
            try await api.closeProductConversation(reference: conversation.aggregateIdentity)
            guard apiGeneration == startedGeneration else { return false }
            closed = true
            return await finalizeProductCloseLocally(
                productConversationId: conversation.aggregateIdentity,
                transcriptIds: transcriptIds,
                startedGeneration: startedGeneration,
                api: api)
        } catch {
            guard apiGeneration == startedGeneration,
                  closeActionGenerations.isCurrent(
                      startedCloseActionGeneration,
                      productConversationId: conversation.aggregateIdentity)
            else { return false }
            if let apiError = error as? APIError,
               apiError.isCloseAlreadyHistory
            {
                closed = true
                return await finalizeProductCloseLocally(
                    productConversationId: conversation.aggregateIdentity,
                    transcriptIds: transcriptIds,
                    startedGeneration: startedGeneration,
                    api: api)
            }
            if let apiError = error as? APIError,
               ["close_stop_work_confirmation_required", "close_loss_confirmation_required"]
                .contains(apiError.serverErrorType),
               let snapshot = try? await api.getProductConversation(
                   reference: conversation.aggregateIdentity),
               apiGeneration == startedGeneration,
               let close = snapshot.close
            {
                pendingProductCloseConfirmation = PendingProductCloseConfirmation(
                    productConversationId: conversation.aggregateIdentity,
                    transcriptRowId: snapshot.latest_transcript_row_id,
                    close: close)
                await listStore.refresh(api: api)
                return false
            }
            lastActionError = error.localizedDescription
            return false
        }
    }

    private func finalizeProductCloseLocally(
        productConversationId: String,
        transcriptIds: Set<String>,
        startedGeneration: Int,
        api: PhoenixAPI
    ) async -> Bool {
        guard apiGeneration == startedGeneration else { return false }
        listStore.projectHistory(aggregateId: productConversationId)
        for transcriptId in transcriptIds {
            sessions.removeValue(forKey: transcriptId)?.stop()
            drainSessions.removeValue(forKey: transcriptId)?.stop()
        }
        do {
            _ = try await loadProductHistory(productConversationId: productConversationId)
        } catch ProductHistoryLoadError.notFound {
            guard apiGeneration == startedGeneration else { return false }
            removeAttentionNotifications(productConversationId: productConversationId)
            return true
        } catch {
            guard apiGeneration == startedGeneration else { return false }
        }
        await listStore.refresh(api: api)
        guard apiGeneration == startedGeneration else { return false }
        listStore.projectHistory(aggregateId: productConversationId)
        removeAttentionNotifications(productConversationId: productConversationId)
        return true
    }

    func resolvePendingProductCloseConfirmation(confirm: Bool) async {
        _ = confirmationRehydrationGenerations.begin(
            productConversationId: "pending-close-confirmation")
        guard connectivity.isOnline else {
            lastActionError = "Resolving a Close confirmation needs a connection — reconnect and try again."
            return
        }
        guard let pending = pendingProductCloseConfirmation,
              let kind = pending.kind,
              let api
        else { return }
        if kind == .losses && confirm,
           (!ProductCloseLossInventory.isComplete(pending.close.losses)
               || pending.close.confirmation_snapshot == nil)
        {
            lastActionError = "Close confirmation is missing its exact retirement loss inventory."
            return
        }
        let startedGeneration = apiGeneration
        let transcriptIds = Set(
            listStore.transcriptRowIds(forAggregateId: pending.productConversationId)
                + [pending.transcriptRowId])
        if confirm {
            guard await requireEmptyAggregateOutboxes(transcriptIds: transcriptIds) else {
                return
            }
        }
        guard apiGeneration == startedGeneration else { return }
        guard let actionGeneration = pendingProductCloseResolution.begin(
            productConversationId: pending.productConversationId)
        else { return }
        defer {
            pendingProductCloseResolution.end(
                actionGeneration, productConversationId: pending.productConversationId)
        }
        do {
            if confirm {
                switch kind {
                case .stopWork:
                    try await api.confirmCloseStopWork(
                        conversationId: pending.transcriptRowId,
                        attemptId: pending.close.attempt_id)
                case .losses:
                    guard let inspection = pending.close.confirmation_snapshot else { return }
                    try await api.confirmCloseLossRetirement(
                        conversationId: pending.transcriptRowId,
                        attemptId: pending.close.attempt_id,
                        inspection: inspection)
                case .repair:
                    try await api.retryCloseRetirement(
                        conversationId: pending.transcriptRowId,
                        attemptId: pending.close.attempt_id)
                }
            } else if kind == .repair {
                pendingProductCloseConfirmation = nil
                return
            } else {
                try await api.cancelClose(
                    conversationId: pending.transcriptRowId,
                    attemptId: pending.close.attempt_id)
            }
            guard isCurrentPendingCloseAction(
                actionGeneration,
                productConversationId: pending.productConversationId,
                apiGeneration: startedGeneration)
            else { return }
            if confirm && kind == .repair {
                let snapshot = try await api.getProductConversation(
                    reference: pending.productConversationId)
                guard isCurrentPendingCloseAction(
                    actionGeneration,
                    productConversationId: pending.productConversationId,
                    apiGeneration: startedGeneration)
                else { return }
                if let refreshed = PendingProductCloseConfirmation(snapshot: snapshot) {
                    pendingProductCloseConfirmation = refreshed
                    await listStore.refresh(api: api)
                } else if PendingProductCloseConfirmation.isCompleted(snapshot: snapshot) {
                    let transcriptIds = Set(
                        listStore.transcriptRowIds(forAggregateId: pending.productConversationId)
                            + [snapshot.latest_transcript_row_id])
                    _ = await finalizeProductCloseLocally(
                        productConversationId: pending.productConversationId,
                        transcriptIds: transcriptIds,
                        startedGeneration: startedGeneration,
                        api: api)
                } else {
                    pendingProductCloseConfirmation = nil
                    await listStore.refresh(api: api)
                }
            } else if confirm {
                pendingProductCloseConfirmation = nil
                _ = await finalizeProductCloseLocally(
                    productConversationId: pending.productConversationId,
                    transcriptIds: transcriptIds,
                    startedGeneration: startedGeneration,
                    api: api)
            } else {
                pendingProductCloseConfirmation = nil
                await listStore.refresh(api: api)
            }
        } catch {
            guard isCurrentPendingCloseAction(
                actionGeneration,
                productConversationId: pending.productConversationId,
                apiGeneration: startedGeneration)
            else { return }
            if let snapshot = try? await api.getProductConversation(
                reference: pending.productConversationId),
               isCurrentPendingCloseAction(
                   actionGeneration,
                   productConversationId: pending.productConversationId,
                   apiGeneration: startedGeneration)
            {
                if PendingProductCloseConfirmation.isCompleted(snapshot: snapshot) {
                    pendingProductCloseConfirmation = nil
                    let reconciledTranscriptIds = Set(
                        transcriptIds + [snapshot.latest_transcript_row_id])
                    _ = await finalizeProductCloseLocally(
                        productConversationId: pending.productConversationId,
                        transcriptIds: reconciledTranscriptIds,
                        startedGeneration: startedGeneration,
                        api: api)
                    return
                } else {
                    pendingProductCloseConfirmation = PendingProductCloseConfirmation(snapshot: snapshot)
                    await listStore.refresh(api: api)
                }
            }
            guard isCurrentPendingCloseAction(
                actionGeneration,
                productConversationId: pending.productConversationId,
                apiGeneration: startedGeneration)
            else { return }
            lastActionError = error.localizedDescription
        }
    }

    private func isCurrentPendingCloseAction(
        _ actionGeneration: Int,
        productConversationId: String,
        apiGeneration startedGeneration: Int
    ) -> Bool {
        apiGeneration == startedGeneration
            && pendingProductCloseResolution.isCurrent(
                actionGeneration, productConversationId: productConversationId)
    }

    @discardableResult
    func deleteHistoryConversation(_ conversation: Conversation) async -> Bool {
        guard ClientOperation.delete.policy == .onlineOnly else { return false }
        let startedGeneration = apiGeneration
        guard let api, connectivity.isOnline else {
            lastActionError = "Deleting needs a connection — it can't be queued."
            return false
        }
        let transcriptIds = Set(
            listStore.transcriptRowIds(forAggregateId: conversation.aggregateIdentity)
                + [conversation.transcriptRowIdentity])
        do {
            let rootTranscriptRowId = conversation.chain_root_id ?? conversation.transcriptRowIdentity
            try await api.deleteProductConversation(rootTranscriptRowId: rootTranscriptRowId)
            guard apiGeneration == startedGeneration else { return false }
            return await removeProductHistoryLocally(
                productConversationId: conversation.aggregateIdentity,
                transcriptIds: transcriptIds,
                startedGeneration: startedGeneration)
        } catch let error as APIError where error.isNotFound {
            return await removeProductHistoryLocally(
                productConversationId: conversation.aggregateIdentity,
                transcriptIds: transcriptIds,
                startedGeneration: startedGeneration)
        } catch {
            guard apiGeneration == startedGeneration else { return false }
            lastActionError = error.localizedDescription
            return false
        }
    }

    private func removeProductHistoryLocally(
        productConversationId: String,
        transcriptIds: Set<String>,
        startedGeneration: Int
    ) async -> Bool {
        guard apiGeneration == startedGeneration else { return false }

        _ = productHistoryGenerations.begin(productConversationId: productConversationId)
        let historyWriter = ProductHistorySnapshotStore.writer(
            productConversationId: productConversationId)
        let revision = historyWriter.reserveRevision()
        await historyWriter.remove(revision: revision)
        guard apiGeneration == startedGeneration else { return false }

        for transcriptId in transcriptIds {
            guard apiGeneration == startedGeneration else { return false }
            let openOwner = sessions.removeValue(forKey: transcriptId)
            let drainOwner = drainSessions.removeValue(forKey: transcriptId)
            let owners = [openOwner, drainOwner].compactMap { $0 }
            owners.forEach { $0.stop() }

            for session in owners {
                guard apiGeneration == startedGeneration else { return false }
                await session.clearCachedSnapshotAndWait()
                guard apiGeneration == startedGeneration else { return false }
                await session.outbox.clearAndWait()
                guard apiGeneration == startedGeneration else { return false }
            }
            DiskStore.remove(name: "conv-\(transcriptId)")
            DiskStore.remove(name: "outbox-\(transcriptId)")
        }

        guard apiGeneration == startedGeneration else { return false }
        listStore.remove(aggregateId: productConversationId)
        deletedProductHistoryIds.insert(productConversationId)
        if pendingProductCloseConfirmation?.productConversationId == productConversationId {
            pendingProductCloseConfirmation = nil
            pendingProductCloseResolution.reset()
        }
        closingProductConversationIds.remove(productConversationId)
        _ = closeActionGenerations.begin(productConversationId: productConversationId)
        _ = confirmationRehydrationGenerations.begin(
            productConversationId: "pending-close-confirmation")
        if pendingOpenConversationId == productConversationId
            || transcriptIds.contains(pendingOpenConversationId ?? "")
        {
            pendingOpenConversationId = nil
        }
        removeAttentionNotifications(productConversationId: productConversationId)
        return true
    }

    #if DEBUG
    func installPendingProductCloseConfirmationForTesting(
        _ pending: PendingProductCloseConfirmation,
        resolving: Bool = false
    ) {
        _ = confirmationRehydrationGenerations.begin(
            productConversationId: "pending-close-confirmation")
        pendingProductCloseConfirmation = pending
        if resolving {
            _ = pendingProductCloseResolution.begin(
                productConversationId: pending.productConversationId)
        }
    }

    func removeProductHistoryLocallyForTesting(
        productConversationId: String,
        transcriptIds: Set<String>
    ) async -> Bool {
        await removeProductHistoryLocally(
            productConversationId: productConversationId,
            transcriptIds: transcriptIds,
            startedGeneration: apiGeneration)
    }

    func handleAggregateHardDeletedForTesting(
        productConversationId: String,
        transcriptIds: [String]
    ) async {
        await handleAggregateHardDeleted(
            ProductConversationDeletionEvent(
                conversation_id: productConversationId,
                deleted_conversation_ids: transcriptIds),
            generation: apiGeneration)
    }
    #endif

    private func removeAttentionNotifications(productConversationId: String) {
        let identifier = "attention-\(productConversationId)"
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [identifier])
        UNUserNotificationCenter.current().removePendingNotificationRequests(withIdentifiers: [identifier])
    }

    @discardableResult
    func archive(conversationId: String) async -> Bool {
        guard ClientOperation.archive.policy == .onlineOnly else { return false }
        let serverIdentifiesCoordinator = conversationId == coordinatorConversationId
            || listStore.conversations.first {
                $0.transcriptRowIdentity == conversationId
            }?.isCoordinator == true
        guard conversationId != coordinatorConversationId, !serverIdentifiesCoordinator else {
            lastActionError = "The Coordinator is a permanent fleet conversation and can't be archived."
            return false
        }
        guard let api, connectivity.isOnline else {
            lastActionError = "Archiving needs a connection — it can't be queued."
            return false
        }
        let hasInMemoryMessages = sessions[conversationId]?.outbox.visibleEntries.isEmpty == false
        guard !hasInMemoryMessages else {
            lastActionError =
                "This conversation has queued or unconfirmed messages. Retry or discard them before archiving."
            return false
        }
        if let session = sessions[conversationId] {
            _ = await session.outbox.flushPersistence()
        }
        switch Outbox.storedContents(conversationId: conversationId) {
        case .empty:
            break
        case .hasVisibleEntries:
            lastActionError =
                "This conversation has queued or unconfirmed messages. Retry or discard them before archiving."
            return false
        case .inaccessible:
            lastActionError =
                "This conversation's queued-message store can't be read by this app version. Upgrade or clear the cache before archiving."
            return false
        }
        guard let session = session(for: conversationId), session.beginArchiving() else {
            lastActionError =
                "This conversation has queued or unconfirmed messages. Retry or discard them before archiving."
            return false
        }
        var archived = false
        defer {
            if !archived { session.endArchiving() }
        }
        do {
            try await api.archive(conversationId: conversationId)
            archived = true
            session.stop()
            await session.clearCachedSnapshotAndWait()
            await session.outbox.clearAndWait()
            sessions[conversationId] = nil
            let aggregateId = listStore.aggregateId(forTranscriptRowId: conversationId)
            if let aggregateId {
                listStore.remove(aggregateId: aggregateId)
            }
            let notificationId = aggregateId ?? conversationId
            UNUserNotificationCenter.current().removeDeliveredNotifications(
                withIdentifiers: ["attention-\(notificationId)"])
            UNUserNotificationCenter.current().removePendingNotificationRequests(
                withIdentifiers: ["attention-\(notificationId)"])
            return true
        } catch {
            lastActionError = (error as? APIError)?.errorDescription
                ?? error.localizedDescription
            return false
        }
    }

    func foregrounded() {
        isForeground = true
        if let api { startAggregateEventStream(api: api, generation: apiGeneration) }
        Task { await reconcileListThenResumeAndDrain() }
    }

    private func locallyOwnedOrdinaryAggregates() -> [String: Set<String>] {
        var owned: [String: Set<String>] = [:]
        func add(aggregateId: String, transcriptId: String?) {
            if let transcriptId { owned[aggregateId, default: []].insert(transcriptId) }
            else { owned[aggregateId, default: []] = owned[aggregateId, default: []] }
        }

        for row in listStore.conversations where !row.isCoordinator {
            add(aggregateId: row.aggregateIdentity, transcriptId: row.transcriptRowIdentity)
            for transcriptId in listStore.transcriptRowIds(forAggregateId: row.aggregateIdentity) {
                add(aggregateId: row.aggregateIdentity, transcriptId: transcriptId)
            }
        }
        for name in DiskStore.listNames(prefix: "product-history-") {
            let aggregateId = String(name.dropFirst("product-history-".count))
            guard let history = cachedProductHistory(productConversationId: aggregateId),
                  history.snapshot.ordinary_lifecycle != nil
            else { continue }
            for segment in history.snapshot.segments {
                add(aggregateId: aggregateId, transcriptId: segment.transcript_row_id)
            }
        }
        let ownedTranscriptIds = Set(sessions.keys)
            .union(drainSessions.keys)
            .union(DiskStore.listNames(prefix: "conv-").map {
                String($0.dropFirst("conv-".count))
            })
        for transcriptId in ownedTranscriptIds {
            let cached = ConversationSession.cachedConversation(conversationId: transcriptId)
            guard cached?.isCoordinator != true else { continue }
            let aggregateId = listStore.aggregateId(forTranscriptRowId: transcriptId)
                ?? cached?.product_conversation_id
            if let aggregateId { add(aggregateId: aggregateId, transcriptId: transcriptId) }
        }
        return owned
    }

    nonisolated static func removedAggregateIds(
        authoritative: [Conversation],
        locallyOwned: Set<String>
    ) -> Set<String> {
        let authoritativeIds = Set(authoritative.lazy.map(\.aggregateIdentity))
        return locallyOwned.subtracting(authoritativeIds)
    }

    private func reconcileListThenResumeAndDrain() async {
        guard let api, connectivity.isOnline else { return }
        let startedGeneration = apiGeneration
        let listToken = listStore.externalRefreshToken()
        let locallyOwned = locallyOwnedOrdinaryAggregates()
        guard let fresh = try? await api.listConversations(),
              !Task.isCancelled,
              apiGeneration == startedGeneration,
              connectivity.isOnline,
              listStore.applyExternal(fresh, startedAt: listToken)
        else { return }

        let removed = Self.removedAggregateIds(
            authoritative: fresh,
            locallyOwned: Set(locallyOwned.keys))
        for aggregateId in removed.sorted() {
            guard await removeProductHistoryLocally(
                productConversationId: aggregateId,
                transcriptIds: locallyOwned[aggregateId] ?? [],
                startedGeneration: startedGeneration)
            else { return }
        }
        guard !Task.isCancelled, apiGeneration == startedGeneration, connectivity.isOnline else {
            return
        }
        await rehydratePendingProductCloseConfirmation(api: api)
        guard apiGeneration == startedGeneration else { return }
        attention.seed(
            with: listStore.conversations,
            transcriptToAggregate: listStore.transcriptToAggregate)
        if isForeground {
            for session in sessions.values { session.resyncAfterForeground() }
        }
        drainPersistedOutboxes()
    }

    func integrateBackgroundConversationUpdate(existing: Conversation, update: Conversation) -> Conversation {
        if let aggregateIdentity = existing.product_conversation_id {
            return mergeAggregateProjection(
                existing: existing,
                liveUpdate: update,
                aggregateIdentity: aggregateIdentity)
        }
        return update
    }

    /// Deliver queued messages for conversations the user hasn't reopened.
    /// After a cold restart `sessions` is empty, so without this sweep an
    /// outbox persisted under `outbox-<id>.json` would sit on disk until
    /// its conversation was opened manually — breaking the restart-survival
    /// half of the offline queue. Sessions created here don't start an SSE
    /// stream; they exist to drain (their outbox reconciles on next open).
    private func drainPersistedOutboxes() {
        guard let api else { return }
        for name in DiskStore.names(withPrefix: "outbox-") {
            let conversationId = String(name.dropFirst("outbox-".count))
            guard !conversationId.isEmpty, sessions[conversationId] == nil else {
                // Open sessions already drain via their own triggers.
                continue
            }
            guard let entries = DiskStore.loadVersioned(
                [OutboxEntry].self, name: name, version: Outbox.schemaVersion),
                  entries.contains(where: { $0.status == .pending && !$0.acceptedByServer })
            else { continue }
            let drainSession: ConversationSession
            if let existing = drainSessions[conversationId] {
                drainSession = existing
            } else {
                drainSession = ConversationSession(
                    conversationId: conversationId, api: api, connectivity: connectivity)
                drainSessions[conversationId] = drainSession
            }
            drainSession.drainOutbox()
        }
    }

    func backgrounded() {
        isForeground = false
        aggregateEventTask?.cancel()
        aggregateEventTask = nil
        // Streams die in the background anyway; stop them cleanly and
        // persist snapshots. Outboxes are already disk-backed.
        for session in sessions.values { session.pauseForBackground() }
        if backgroundNudgesEnabled {
            BackgroundRefresh.scheduleNext()
        }
    }

    /// Sign-out also clears all cached data: conversations, the last-used
    /// working directory, and the pinned certificate are per-server state
    /// and must not leak across a server/account switch.
    func signOut() async {
        nudgePreferenceGeneration &+= 1
        backgroundNudgesEnabled = false
        nudgeAuthorizationHint = nil
        UserDefaults.standard.removeObject(forKey: Self.nudgesEnabledKey)
        BackgroundRefresh.cancelPending()
        await clearCache()
        pendingOpenConversationId = nil
        let notificationCenter = UNUserNotificationCenter.current()
        notificationCenter.removeAllDeliveredNotifications()
        notificationCenter.removeAllPendingNotificationRequests()
        UserDefaults.standard.removeObject(forKey: Self.lastCwdKey)
        UserDefaults.standard.removeObject(forKey: Self.coordinatorIdKey)
        coordinatorConversationId = nil
        CertPinStore.forget()
        password = ""
        Keychain.deletePassword(account: Self.passwordAccount)
        serverURLString = ""
    }

    func clearCache() async {
        apiGeneration += 1
        aggregateEventTask?.cancel()
        aggregateEventTask = nil
        let ownedSessions = Array(sessions.values) + Array(drainSessions.values)
        for session in ownedSessions { session.stop() }
        for session in ownedSessions { await session.clearCachedSnapshotAndWait() }
        for session in ownedSessions { await session.outbox.clearAndWait() }
        sessions.removeAll()
        drainSessions.removeAll()
        pendingProductCloseConfirmation = nil
        pendingProductCloseResolution.reset()
        closeActionGenerations.reset()
        productHistoryGenerations.reset()
        confirmationRehydrationGenerations.reset()
        closingProductConversationIds.removeAll()
        await DiskStore.removeAllAndWait()
        listStore.reset()
        deletedProductHistoryIds.removeAll()
        attention.reset()
        UserDefaults.standard.removeObject(forKey: Self.coordinatorIdKey)
        coordinatorConversationId = nil
    }

    #if DEBUG
    static func resetPersistentStateForUITesting() {
        if let bundleIdentifier = Bundle.main.bundleIdentifier {
            UserDefaults.standard.removePersistentDomain(forName: bundleIdentifier)
        }
        Keychain.deletePassword(account: Self.passwordAccount)
        DiskStore.removeAll()
        let center = UNUserNotificationCenter.current()
        center.removeAllDeliveredNotifications()
        center.removeAllPendingNotificationRequests()
    }
    #endif
}

/// Routes notification taps into the app (deep link to the conversation)
/// and suppresses banners while the app is foregrounded — the user is
/// already looking at live state.
final class NotificationRouter: NSObject, UNUserNotificationCenterDelegate {
    weak var model: AppModel?

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let conversationId =
            response.notification.request.content.userInfo["conversationId"] as? String
        Task { @MainActor [weak model] in
            if let conversationId {
                model?.pendingOpenConversationId = conversationId
            }
            completionHandler()
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([])
    }
}
