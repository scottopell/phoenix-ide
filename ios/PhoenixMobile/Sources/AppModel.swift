import Foundation
import Observation
import UserNotifications

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

    /// Sessions for conversations the user has opened, kept alive so their
    /// outboxes continue draining while the user navigates elsewhere.
    private var sessions: [String: ConversationSession] = [:]
    /// Short-lived delivery owners for persisted outboxes whose conversation
    /// is not open. Retaining one per conversation serializes every trigger
    /// through the session's single drain task.
    private var drainSessions: [String: ConversationSession] = [:]

    init() {
        serverURLString = UserDefaults.standard.string(forKey: Self.serverURLKey) ?? ""
        password = Keychain.password(account: Self.passwordAccount) ?? ""
        trustSelfSigned = UserDefaults.standard.object(forKey: Self.trustSelfSignedKey) as? Bool ?? true
        rebuildAPI()
        adoptCoordinatorIdentityFromList()
        _ = connectivity.addRestoreObserver { [weak self] in
            self?.drainPersistedOutboxes()
            Task { await self?.refreshList() }
        }
        notificationRouter.model = self
        UNUserNotificationCenter.current().delegate = notificationRouter
    }

    private func rebuildAPI() {
        apiGeneration += 1
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
            self?.listStore.upsert(conversation)
        }
        let onHardDeleted: (String) -> Void = { [weak self] deletedId in
            self?.handleHardDeleted(deletedId)
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

    private func handleHardDeleted(_ conversationId: String) {
        listStore.remove(id: conversationId)
        if pendingOpenConversationId == conversationId {
            pendingOpenConversationId = nil
        }
        UNUserNotificationCenter.current().removeDeliveredNotifications(
            withIdentifiers: ["attention-\(conversationId)"])
    }

    func refreshList() async {
        guard let api else { return }
        await listStore.refresh(api: api)
        if listStore.lastError == nil {
            adoptCoordinatorIdentityFromList()
            // The user is looking at fresh data — nothing here should nudge
            // them later.
            attention.seed(with: listStore.conversations)
        }
    }

    // MARK: - Needs-attention nudges

    let attention = AttentionMonitor()
    private let notificationRouter = NotificationRouter()
    private static let nudgesEnabledKey = "phoenix.backgroundNudges"

    private(set) var backgroundNudgesEnabled =
        UserDefaults.standard.bool(forKey: AppModel.nudgesEnabledKey)
    private(set) var nudgeAuthorizationHint: String?
    /// Set by a notification tap; the list view navigates and clears it.
    var pendingOpenConversationId: String?

    func setBackgroundNudges(_ enabled: Bool) async {
        nudgeAuthorizationHint = nil
        if enabled {
            guard await AttentionMonitor.requestAuthorization() else {
                nudgeAuthorizationHint =
                    "Notifications are off for Phoenix in iOS Settings — enable them there first."
                backgroundNudgesEnabled = false
                UserDefaults.standard.set(false, forKey: Self.nudgesEnabledKey)
                return
            }
        }
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
        attention.checkAndNotify(fresh)
        return true
    }

    // MARK: - Coordinator

    /// The fleet Coordinator's conversation id, remembered across launches
    /// so its cached transcript opens offline and its list row is badged.
    /// Per-server state — cleared on sign-out.
    private(set) var coordinatorConversationId: String? =
        UserDefaults.standard.string(forKey: AppModel.coordinatorIdKey)

    private func adoptCoordinatorIdentityFromList() {
        guard let coordinator = listStore.conversations.first(where: \.isCoordinator) else {
            return
        }
        coordinatorConversationId = coordinator.id
        UserDefaults.standard.set(coordinator.id, forKey: Self.coordinatorIdKey)
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
                   let cached = coordinatorConversationId {
                    return cached
                }
                lastActionError = (error as? APIError)?.errorDescription
                    ?? error.localizedDescription
                return nil
            }
        }
        if let cached = coordinatorConversationId { return cached }
        lastActionError = "Opening the Coordinator for the first time needs a connection."
        return nil
    }

    /// Online-only archive. Returns false with `lastActionError` on failure.
    var lastActionError: String?

    @discardableResult
    func archive(conversationId: String) async -> Bool {
        guard ClientOperation.archive.policy == .onlineOnly else { return false }
        let serverIdentifiesCoordinator = listStore.conversations.first {
            $0.id == conversationId
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
            sessions[conversationId] = nil
            listStore.remove(id: conversationId)
            DiskStore.remove(name: "outbox-\(conversationId)")
            DiskStore.remove(name: "conv-\(conversationId)")
            UNUserNotificationCenter.current().removeDeliveredNotifications(
                withIdentifiers: ["attention-\(conversationId)"])
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
        drainPersistedOutboxes()
        Task { await refreshList() }
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
    func signOut() {
        clearCache()
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

    func clearCache() {
        for session in sessions.values { session.stop() }
        sessions.removeAll()
        for session in drainSessions.values { session.stop() }
        drainSessions.removeAll()
        DiskStore.removeAll()
        listStore.reset()
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
