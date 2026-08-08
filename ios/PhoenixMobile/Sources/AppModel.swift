import Foundation
import Observation

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
        _ = connectivity.addRestoreObserver { [weak self] in
            self?.drainPersistedOutboxes()
            Task { await self?.refreshList() }
        }
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
        for session in drainSessions.values { session.stop() }
        drainSessions.removeAll()
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
            self?.sessions[deletedId] = nil
            self?.listStore.remove(id: deletedId)
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

    func refreshList() async {
        guard let api else { return }
        await listStore.refresh(api: api)
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
            guard let entries = DiskStore.load([OutboxEntry].self, name: name),
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
    }

    /// Sign-out also clears all cached data: conversations, the last-used
    /// working directory, and the pinned certificate are per-server state
    /// and must not leak across a server/account switch.
    func signOut() {
        clearCache()
        UserDefaults.standard.removeObject(forKey: Self.lastCwdKey)
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
    }
}
