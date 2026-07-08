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
    }

    func foregrounded() {
        for session in sessions.values {
            session.resyncAfterForeground()
        }
        Task { await refreshList() }
    }

    func backgrounded() {
        // Streams die in the background anyway; stop them cleanly and
        // persist snapshots. Outboxes are already disk-backed.
        for session in sessions.values { session.stop() }
    }

    /// Sign-out also clears all cached data: conversations are per-server
    /// state and must not leak across a server/account switch.
    func signOut() {
        clearCache()
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
