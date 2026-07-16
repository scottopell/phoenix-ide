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

    var password: String {
        didSet {
            Keychain.setPassword(password, account: Self.passwordAccount)
            rebuildAPI()
        }
    }

    var trustSelfSigned: Bool {
        didSet {
            UserDefaults.standard.set(trustSelfSigned, forKey: Self.trustSelfSignedKey)
            rebuildAPI()
        }
    }

    var isConfigured: Bool {
        URL(string: serverURLString)?.host != nil
    }

    // MARK: - Services

    let connectivity = ConnectivityMonitor()
    let listStore = ConversationListStore()
    private(set) var api: PhoenixAPI?

    /// Sessions for conversations the user has opened, kept alive so their
    /// outboxes continue draining while the user navigates elsewhere.
    private var sessions: [String: ConversationSession] = [:]

    init() {
        serverURLString = UserDefaults.standard.string(forKey: Self.serverURLKey) ?? ""
        password = Keychain.password(account: Self.passwordAccount) ?? ""
        trustSelfSigned = UserDefaults.standard.object(forKey: Self.trustSelfSignedKey) as? Bool ?? true
        rebuildAPI()
        _ = connectivity.addRestoreObserver { [weak self] in
            self?.drainPersistedOutboxes()
        }
        notificationRouter.model = self
        UNUserNotificationCenter.current().delegate = notificationRouter
    }

    private func rebuildAPI() {
        guard let url = URL(string: serverURLString), url.host != nil else {
            api = nil
            return
        }
        api = PhoenixAPI(
            baseURL: url,
            password: password.isEmpty ? nil : password,
            allowSelfSigned: trustSelfSigned)
        // Existing sessions hold the old client; drop them so reopened
        // conversations pick up the new settings. Their outboxes are disk-
        // backed, so nothing is lost.
        for session in sessions.values { session.stop() }
        sessions.removeAll()
    }

    func session(for conversationId: String) -> ConversationSession? {
        guard let api else { return nil }
        if let existing = sessions[conversationId] { return existing }
        let session = ConversationSession(
            conversationId: conversationId, api: api, connectivity: connectivity)
        sessions[conversationId] = session
        return session
    }

    func refreshList() async {
        guard let api else { return }
        await listStore.refresh(api: api)
        if listStore.lastError == nil {
            // The user is looking at fresh data — nothing here should nudge
            // them later.
            attention.seed(with: listStore.conversations)
        }
    }

    // MARK: - Needs-attention nudges (STOPGAP tier — see AttentionMonitor)

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
        }
    }

    /// One background-fetch cycle: fetch the list, notify on attention
    /// transitions, and opportunistically freshen the cached list so the
    /// next cold open is newer. Returns success for BGTask accounting.
    func runBackgroundAttentionCheck() async -> Bool {
        guard backgroundNudgesEnabled, let api else { return false }
        guard let fresh = try? await api.listConversations() else { return false }
        guard !Task.isCancelled else { return false }
        await attention.checkAndNotify(fresh)
        listStore.applyExternal(fresh)
        return true
    }

    // MARK: - Coordinator

    /// The fleet Coordinator's conversation id, remembered across launches
    /// so its cached transcript opens offline and its list row is badged.
    /// Per-server state — cleared on sign-out.
    private(set) var coordinatorConversationId: String? =
        UserDefaults.standard.string(forKey: AppModel.coordinatorIdKey)

    /// Resolve the Coordinator conversation to open. Online: get-or-create
    /// on the server (it's an ordinary conversation; everything downstream
    /// is the normal conversation surface). Offline: fall back to the
    /// remembered id so the cached transcript still opens — asking new
    /// questions then queues through the outbox like any conversation.
    func openCoordinator() async -> String? {
        if let api, connectivity.isOnline {
            do {
                let conversation = try await api.ensureCoordinator()
                coordinatorConversationId = conversation.id
                UserDefaults.standard.set(conversation.id, forKey: Self.coordinatorIdKey)
                listStore.upsert(conversation)
                return conversation.id
            } catch {
                lastActionError = (error as? APIError)?.errorDescription
                    ?? error.localizedDescription
                return nil
            }
        }
        if let cached = coordinatorConversationId { return cached }
        lastActionError = "Opening the Coordinator for the first time needs a connection."
        return nil
    }

    /// List-scoped conversation action (see ConversationAction's policy
    /// doc — archive is online-only: it transitions live server state and
    /// frees resources, so a queued stale archive must not replay later).
    /// Returns false (with `lastActionError` set) on failure so callers
    /// can leave the row in place.
    var lastActionError: String?

    @discardableResult
    func archive(conversationId: String) async -> Bool {
        guard let api, connectivity.isOnline else {
            lastActionError = "Archiving needs a connection — it can't be queued."
            return false
        }
        do {
            try await api.archive(conversationId: conversationId)
            sessions[conversationId]?.stop()
            sessions[conversationId] = nil
            listStore.remove(id: conversationId)
            // Archiving abandons the conversation's local state, including
            // any queued drafts: the server rejects chat to archived
            // conversations, so a surviving outbox file would only feed the
            // drain sweep undeliverable text (or an unreachable failure).
            DiskStore.remove(name: "outbox-\(conversationId)")
            DiskStore.remove(name: "conv-\(conversationId)")
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
        guard api != nil else { return }
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
            session(for: conversationId)?.drainOutbox()
        }
    }

    func backgrounded() {
        // Streams die in the background anyway; stop them cleanly and
        // persist snapshots. Outboxes are already disk-backed.
        for session in sessions.values { session.stop() }
        if backgroundNudgesEnabled {
            BackgroundRefresh.scheduleNext()
        }
    }

    /// Sign-out also clears all cached data: conversations, the last-used
    /// working directory, and the pinned certificate are per-server state
    /// and must not leak across a server/account switch.
    func signOut() {
        clearCache()
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
        DiskStore.removeAll()
        listStore.reset()
    }
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
