import Foundation
import Observation
import UserNotifications

/// Root composition: server settings, connectivity, API client, stores, and
/// the active per-conversation sessions.
private let coordinatorIdentityDefaultsKey = "phoenix.coordinatorConversationId"

@MainActor
protocol CoordinatorIdentityStore {
    func load() -> String?
    func save(_ conversationId: String)
    func clear()
}

@MainActor
struct UserDefaultsCoordinatorIdentityStore: CoordinatorIdentityStore {
    func load() -> String? {
        UserDefaults.standard.string(forKey: coordinatorIdentityDefaultsKey)
    }

    func save(_ conversationId: String) {
        UserDefaults.standard.set(conversationId, forKey: coordinatorIdentityDefaultsKey)
    }

    func clear() {
        UserDefaults.standard.removeObject(forKey: coordinatorIdentityDefaultsKey)
    }
}

@MainActor
protocol PersistedOutboxStore {
    func visibleOwnerTranscriptRowIds() -> Set<String>
    func loadContents(conversationId: String) -> PersistedOutboxStoreContents
}

@MainActor
enum PersistedOutboxStoreContents: Equatable {
    case missing
    case entries([OutboxEntry])
    case inaccessible
}

@MainActor
struct DiskPersistedOutboxStore: PersistedOutboxStore {
    func visibleOwnerTranscriptRowIds() -> Set<String> {
        Set(DiskStore.names(withPrefix: "outbox-").compactMap { name in
            guard name.hasPrefix("outbox-") else { return nil }
            let conversationId = String(name.dropFirst("outbox-".count))
            guard !conversationId.isEmpty else { return nil }
            switch Outbox.storedContents(conversationId: conversationId) {
            case .hasVisibleEntries:
                return conversationId
            case .empty, .inaccessible:
                return nil
            }
        })
    }

    func loadContents(conversationId: String) -> PersistedOutboxStoreContents {
        switch DiskStore.loadVersionedResult([OutboxEntry].self, name: "outbox-\(conversationId)", version: Outbox.schemaVersion) {
        case .missing:
            return .missing
        case .value(let entries):
            return .entries(entries.filter { $0.conversationId == conversationId && $0.isVisible })
        case .incompatible, .unreadable:
            return .inaccessible
        }
    }
}

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

    var configurationIdentity: String {
        "\(apiGeneration)|\(serverURLString)|\(trustSelfSigned)"
    }

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
    let listStore: ConversationListStore
    private(set) var api: PhoenixAPI?
    /// Invalidates responses started with earlier server credentials or URL.
    private var apiGeneration = 0

    /// Sessions for conversations the user has opened, kept alive so their
    /// outboxes continue draining while the user navigates elsewhere.
    private var sessions: [String: ConversationSession] = [:]
    /// Short-lived delivery owners for persisted outboxes whose conversation
    /// is not open. Retaining one per conversation serializes every trigger
    /// through the session's single drain task.
    private var drainSessions: [String: ConversationSession] = [:]
    private let hasCachedSnapshot: (String) -> Bool
    private let persistedOutboxStore: PersistedOutboxStore
    private let coordinatorIdentityStore: CoordinatorIdentityStore
    private var persistedOutboxHydrated = false
    private var startupDrainGeneration = 0
    private var lastCompletedDrainGeneration = 0
    private var persistedOutboxDrainTask: Task<Void, Never>?
    private var persistedOutboxDrainTaskGeneration: Int?

    init(
        hasCachedSnapshot: ((String) -> Bool)? = nil,
        persistedOutboxStore: PersistedOutboxStore? = nil,
        coordinatorIdentityStore: CoordinatorIdentityStore? = nil
    ) {
        self.hasCachedSnapshot = hasCachedSnapshot ?? { conversationId in
            MainActor.assumeIsolated {
                ConversationSession.hasCachedSnapshot(conversationId: conversationId)
            }
        }
        self.persistedOutboxStore = persistedOutboxStore ?? DiskPersistedOutboxStore()
        productConversationDetails = [:]
        self.coordinatorIdentityStore = coordinatorIdentityStore ?? UserDefaultsCoordinatorIdentityStore()
        listStore = ConversationListStore(hasCachedSnapshot: self.hasCachedSnapshot)
        serverURLString = UserDefaults.standard.string(forKey: Self.serverURLKey) ?? ""
        password = Keychain.password(account: Self.passwordAccount) ?? ""
        trustSelfSigned = UserDefaults.standard.object(forKey: Self.trustSelfSignedKey) as? Bool ?? true
        attention = AttentionMonitor(
            currentConversations: listStore.conversations,
            transcriptToAggregate: listStore.transcriptToAggregate)
        rebuildAPI()
        _ = connectivity.addRestoreObserver { [weak self] in
            self?.schedulePersistedOutboxDrain()
            Task { await self?.refreshList() }
        }
        notificationRouter.model = self
        UNUserNotificationCenter.current().delegate = notificationRouter
        coordinatorConversationId = self.coordinatorIdentityStore.load()
        finishStartupHydration()
    }

    private func finishStartupHydration() {
        persistedOutboxHydrated = true
        schedulePersistedOutboxDrain()
    }

    private func schedulePersistedOutboxDrain() {
        startupDrainGeneration &+= 1
        triggerPersistedOutboxDrainIfNeeded()
    }

    private func triggerPersistedOutboxDrainIfNeeded() {
        guard persistedOutboxHydrated,
              connectivity.isOnline,
              api != nil,
              persistedOutboxDrainTask == nil,
              lastCompletedDrainGeneration < startupDrainGeneration
        else { return }
        let generation = startupDrainGeneration
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.runPersistedOutboxDrain(generation: generation)
        }
        persistedOutboxDrainTaskGeneration = generation
        persistedOutboxDrainTask = task
    }

    private func runPersistedOutboxDrain(generation: Int) async {
        let drainedConversationIds = Set(drainPersistedOutboxes())
        let joinedConversationIds = drainedConversationIds.union(Set(sessions.keys))
        for conversationId in joinedConversationIds {
            let session = drainSessions[conversationId] ?? sessions[conversationId]
            guard let session,
                  let drainGeneration = session.drainOutbox()
            else { continue }
            _ = await session.awaitDrainOutbox(generation: drainGeneration)
            _ = await session.outbox.flushPersistence()
        }
        lastCompletedDrainGeneration = max(lastCompletedDrainGeneration, generation)
        if persistedOutboxDrainTaskGeneration == generation {
            persistedOutboxDrainTask = nil
            persistedOutboxDrainTaskGeneration = nil
        }
        triggerPersistedOutboxDrainIfNeeded()
    }

    private func rebuildAPI() {
        apiGeneration += 1
        let configuredAPI: PhoenixAPI?
        if let url = URL(string: serverURLString), url.host != nil {
            configuredAPI = PhoenixAPI(
                baseURL: url,
                password: password.isEmpty ? nil : password,
                allowSelfSigned: trustSelfSigned)
        } else {
            configuredAPI = nil
        }
        api = configuredAPI
        for session in sessions.values { session.invalidateConfiguration() }
        for session in drainSessions.values { session.invalidateConfiguration() }
        let cachedDetails = Array(productConversationDetails.values)
        productConversationDetails.removeAll()
        for detail in cachedDetails {
            detail.invalidateConfiguration()
        }
        guard let configuredAPI else { return }
        for session in sessions.values { session.replaceAPI(configuredAPI) }
        for session in drainSessions.values { session.replaceAPI(configuredAPI) }
        schedulePersistedOutboxDrain()
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

    private func handleAggregateHardDeleted(aggregateId: String, transcriptRowId: String?) async {
        let aggregateConversationIds = Set(listStore.transcriptToAggregate.compactMap { key, value in
            value == aggregateId ? key : nil
        }).union(Set([transcriptRowId].compactMap { $0 }))
        let ownedSessions = aggregateConversationIds.compactMap { sessions.removeValue(forKey: $0) }
        let ownedDrainSessions = aggregateConversationIds.compactMap { drainSessions.removeValue(forKey: $0) }
        for session in ownedSessions { session.stop() }
        for session in ownedDrainSessions { session.stop() }
        productConversationDetails[aggregateId]?.stop()
        productConversationDetails.removeValue(forKey: aggregateId)
        listStore.remove(aggregateId: aggregateId)
        if pendingOpenConversationId == transcriptRowId || pendingOpenConversationId == aggregateId {
            pendingOpenConversationId = nil
        }
        UNUserNotificationCenter.current().removeDeliveredNotifications(
            withIdentifiers: ["attention-\(aggregateId)"])
        UNUserNotificationCenter.current().removePendingNotificationRequests(
            withIdentifiers: ["attention-\(aggregateId)"])
        for session in ownedSessions {
            await session.clearCachedSnapshotAndWait()
            await session.outbox.clearAndWait()
        }
        for session in ownedDrainSessions {
            await session.clearCachedSnapshotAndWait()
            await session.outbox.clearAndWait()
        }
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

    func refreshList() async {
        guard let api else { return }
        attentionEvidenceGeneration &+= 1
        await listStore.refresh(api: api)
        if listStore.lastError == nil {
            // The user is looking at fresh data — nothing here should nudge
            // them later.
            attention.seed(
                with: listStore.conversations,
                transcriptToAggregate: listStore.transcriptToAggregate)
        }
    }

    // MARK: - Needs-attention nudges

    private var productConversationDetails: [String: ProductConversationDetailModel] = [:]
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

    func navigationConversationId(for conversation: Conversation) -> String {
        resolvedNavigationConversationId(
            aggregateId: conversation.product_conversation_id,
            latestTranscriptRowId: conversation.transcriptRowIdentity)
    }

    func existingSession(for conversationId: String) -> ConversationSession? {
        if let existing = sessions[conversationId] { return existing }
        return drainSessions[conversationId]
    }

    func configureForTesting(serverURL: String, password: String = "", trustSelfSigned: Bool = true) {
        self.password = password
        self.trustSelfSigned = trustSelfSigned
        serverURLString = serverURL
    }

    enum PersistedOutboxDrainAwaitResult: Equatable {
        case completed(Int)
        case noCurrentDrain
        case notReady
    }

    func currentPersistedOutboxDrainGenerationForTesting() -> Int? {
        persistedOutboxDrainTaskGeneration
    }

    func awaitPersistedOutboxDrainForTesting(generation: Int) async -> PersistedOutboxDrainAwaitResult {
        if lastCompletedDrainGeneration >= generation {
            return .completed(generation)
        }
        if !persistedOutboxHydrated || api == nil || !connectivity.isOnline {
            return .notReady
        }
        guard persistedOutboxDrainTaskGeneration == generation,
              let task = persistedOutboxDrainTask
        else { return .noCurrentDrain }
        await task.value
        return lastCompletedDrainGeneration >= generation ? .completed(generation) : .noCurrentDrain
    }

    func awaitCurrentPersistedOutboxDrainForTesting() async -> PersistedOutboxDrainAwaitResult {
        guard let generation = persistedOutboxDrainTaskGeneration else {
            if !persistedOutboxHydrated || api == nil || !connectivity.isOnline {
                return .notReady
            }
            return .noCurrentDrain
        }
        return await awaitPersistedOutboxDrainForTesting(generation: generation)
    }

    func persistedOutboxContents(for conversationId: String) -> Outbox.StoredContents {
        switch persistedOutboxStore.loadContents(conversationId: conversationId) {
        case .entries(let entries):
            return entries.contains(where: { $0.isVisible }) ? .hasVisibleEntries : .empty
        case .missing:
            return .empty
        case .inaccessible:
            return .inaccessible
        }
    }

    func productConversationDetailModel(
        for aggregateId: String,
        initialTranscriptRowId: String? = nil
    ) -> ProductConversationDetailModel {
        if let existing = productConversationDetails[aggregateId] {
            if initialTranscriptRowId != nil {
                existing.primeInitialTranscriptRowId(initialTranscriptRowId)
            }
            return existing
        }
        guard let api else {
            fatalError("ProductConversationDetailModel requires configured API")
        }
        let created = ProductConversationDetailModel(
            aggregateId: aggregateId,
            initialTranscriptRowId: initialTranscriptRowId,
            api: api,
            connectivity: connectivity,
            sessionProvider: { [weak self] transcriptRowId in
                self?.session(for: transcriptRowId)
            },
            existingSession: { [weak self] transcriptRowId in
                self?.existingSession(for: transcriptRowId)
            },
            persistedOutboxContents: { [weak self] transcriptRowId in
                self?.persistedOutboxContents(for: transcriptRowId) ?? .empty
            },
            hasCachedSnapshot: { [weak self] transcriptRowId in
                self?.hasCachedSnapshot(transcriptRowId) ?? false
            },
            handleDefinitiveNotFound: { [weak self] transcriptRowId in
                await self?.handleAggregateHardDeleted(aggregateId: aggregateId, transcriptRowId: transcriptRowId)
            },
            onConfigurationInvalidated: { [weak self] in
                self?.productConversationDetails.removeValue(forKey: aggregateId)
            })
        productConversationDetails[aggregateId] = created
        return created
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
        guard !Task.isCancelled,
              backgroundNudgesEnabled,
              apiGeneration == startedGeneration
        else { return false }
        guard !Task.isCancelled,
              backgroundNudgesEnabled,
              apiGeneration == startedGeneration,
              listStore.canApplyExternal(startedAt: listToken)
        else { return false }
        guard listStore.applyExternal(fresh, startedAt: listToken) else { return false }
        let isCurrent: @MainActor () -> Bool = { [weak self] in
            guard let self else { return false }
            return self.backgroundNudgesEnabled
                && self.apiGeneration == startedGeneration
                && self.nudgePreferenceGeneration == startedNudgeGeneration
                && self.attentionEvidenceGeneration == startedEvidenceGeneration
        }
        await attention.refreshAndNotifyIfNeeded(
            from: fresh,
            transcriptToAggregate: listStore.transcriptToAggregate,
            isCurrent: isCurrent)
        return await isCurrent()
    }

    // MARK: - Coordinator

    /// The fleet Coordinator's conversation id, remembered across launches
    /// so its cached transcript opens offline and its list row is badged.
    /// Per-server state — cleared on sign-out.
    private(set) var coordinatorConversationId: String?

    var coordinatorAvailableOffline: Bool {
        guard let id = coordinatorConversationId else { return false }
        return hasCachedSnapshot(id)
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
                coordinatorIdentityStore.save(conversation.id)
                listStore.upsert(conversation)
                return conversation.id
            } catch {
                guard !Task.isCancelled, apiGeneration == startedGeneration else { return nil }
                if let apiError = error as? APIError,
                   apiError.isTransport,
                   let cached = coordinatorConversationId,
                   hasCachedSnapshot(cached) {
                    return cached
                }
                lastActionError = (error as? APIError)?.errorDescription
                    ?? error.localizedDescription
                return nil
            }
        }
        if let cached = coordinatorConversationId,
           hasCachedSnapshot(cached) {
            return cached
        }
        lastActionError = "Opening the Coordinator offline needs a cached conversation."
        return nil
    }

    /// Online-only archive. Returns false with `lastActionError` on failure.
    var lastActionError: String?

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
        switch persistedOutboxContents(for: conversationId) {
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
        for session in sessions.values {
            session.resyncAfterForeground()
        }
        schedulePersistedOutboxDrain()
        Task { await refreshList() }
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
    @discardableResult
    private func drainPersistedOutboxes() -> [String] {
        guard let api else { return [] }
        var drainedConversationIds: [String] = []
        for conversationId in persistedOutboxStore.visibleOwnerTranscriptRowIds().sorted() {
            guard sessions[conversationId] == nil else {
                // Open sessions already drain via their own triggers.
                continue
            }
            guard case .entries(let entries) = persistedOutboxStore.loadContents(conversationId: conversationId),
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
            drainedConversationIds.append(conversationId)
        }
        return drainedConversationIds
    }

    func backgrounded() {
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
        coordinatorIdentityStore.clear()
        coordinatorConversationId = nil
        CertPinStore.forget()
        password = ""
        Keychain.deletePassword(account: Self.passwordAccount)
        serverURLString = ""
    }

    func clearCache() async {
        productConversationDetails.removeAll()
        apiGeneration += 1
        let ownedSessions = Array(sessions.values) + Array(drainSessions.values)
        for session in ownedSessions { session.stop() }
        for session in ownedSessions { await session.clearCachedSnapshotAndWait() }
        for session in ownedSessions { await session.outbox.clearAndWait() }
        sessions.removeAll()
        drainSessions.removeAll()
        await DiskStore.removeAllAndWait()
        listStore.reset()
        attention.reset()
        coordinatorIdentityStore.clear()
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
