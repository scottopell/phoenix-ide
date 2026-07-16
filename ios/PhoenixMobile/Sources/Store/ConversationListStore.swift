import Foundation
import Observation

/// Read-through cache of the conversation list. The cached copy renders
/// instantly (including with no connectivity); a refresh replaces it when
/// the server answers.
@MainActor
@Observable
final class ConversationListStore {
    private(set) var conversations: [Conversation] = []
    private(set) var lastRefreshed: Date?
    var isRefreshing: Bool { refreshToken != nil }
    private(set) var lastError: String?

    private static let cacheName = "conversations"
    private static let schemaVersion = 1

    private struct Cache: Codable {
        var conversations: [Conversation]
        var lastRefreshed: Date
    }

    /// Reset invalidates an in-flight refresh. Row-level changes are folded
    /// into its result so the refresh can still update unrelated rows.
    private var generation = 0
    private var refreshToken: UUID?
    /// Server-pushed rows received after the current full refresh began.
    private var upsertsDuringRefresh: [String: Conversation] = [:]
    /// Rows removed or archived after the current full refresh began.
    private var exclusionsDuringRefresh: Set<String> = []

    init() {
        if let cache = DiskStore.loadVersioned(
            Cache.self, name: Self.cacheName, version: Self.schemaVersion) {
            conversations = cache.conversations
            lastRefreshed = cache.lastRefreshed
        }
    }

    func refresh(api: PhoenixAPI) async {
        guard refreshToken == nil else { return }
        let token = UUID()
        refreshToken = token
        defer {
            if refreshToken == token {
                refreshToken = nil
                upsertsDuringRefresh.removeAll()
                exclusionsDuringRefresh.removeAll()
            }
        }
        let startedGeneration = generation
        do {
            let fresh = try await api.listConversations()
            guard generation == startedGeneration else { return }
            apply(Self.merging(
                fresh,
                preserving: upsertsDuringRefresh,
                excluding: exclusionsDuringRefresh))
            lastError = nil
        } catch {
            guard generation == startedGeneration else { return }
            // Keep the cached list — stale data beats no data offline.
            lastError = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    nonisolated static func merging(
        _ fresh: [Conversation],
        preserving upserts: [String: Conversation],
        excluding exclusions: Set<String> = []
    ) -> [Conversation] {
        var byId = Dictionary(uniqueKeysWithValues: fresh
            .filter { $0.archived != true && !exclusions.contains($0.id) }
            .map { ($0.id, $0) })
        for (id, conversation) in upserts
        where conversation.archived != true && !exclusions.contains(id) {
            byId[id] = conversation
        }
        return Array(byId.values)
    }

    private func apply(_ fresh: [Conversation]) {
        conversations = Self.sortedByUpdatedAt(fresh)
        lastRefreshed = Date()
        persistCache()
    }

    /// Apply an externally fetched list (background attention check) so a
    /// cold open renders fresher data. Skipped while a foreground refresh
    /// is in flight; the generation guard semantics match refresh().
    func applyExternal(_ fresh: [Conversation]) {
        guard !isRefreshing else { return }
        apply(fresh)
        lastError = nil
    }

    /// Merge a single updated conversation (e.g. after creation or an SSE
    /// update in an open session) without waiting for a full refresh.
    func upsert(_ conversation: Conversation) {
        if lastRefreshed == nil { lastRefreshed = Date() }
        if conversation.archived == true {
            if isRefreshing {
                upsertsDuringRefresh[conversation.id] = nil
                exclusionsDuringRefresh.insert(conversation.id)
            }
            conversations.removeAll { $0.id == conversation.id }
            persistCache()
            return
        }
        if isRefreshing {
            exclusionsDuringRefresh.remove(conversation.id)
            upsertsDuringRefresh[conversation.id] = conversation
        }
        if let idx = conversations.firstIndex(where: { $0.id == conversation.id }) {
            conversations[idx] = conversation
        } else {
            conversations.insert(conversation, at: 0)
        }
        conversations = Self.sortedByUpdatedAt(conversations)
        persistCache()
    }

    nonisolated static func sortedByUpdatedAt(_ conversations: [Conversation]) -> [Conversation] {
        conversations
            .map { (conversation: $0, updatedAt: $0.updatedAtDate ?? .distantPast) }
            .sorted { $0.updatedAt > $1.updatedAt }
            .map(\.conversation)
    }

    func remove(id: String) {
        upsertsDuringRefresh[id] = nil
        if isRefreshing {
            exclusionsDuringRefresh.insert(id)
        }
        conversations.removeAll { $0.id == id }
        persistCache()
    }

    /// Drop in-memory state after the disk cache is cleared (or the user
    /// signs out). Without this the long-lived store keeps showing
    /// supposedly-deleted rows until a successful refresh. Also invalidates
    /// any in-flight refresh so its late response can't repopulate the
    /// cleared cache with the previous server's data.
    func reset() {
        generation += 1
        refreshToken = nil
        upsertsDuringRefresh.removeAll()
        exclusionsDuringRefresh.removeAll()
        conversations = []
        lastRefreshed = nil
        lastError = nil
    }

    private func persistCache() {
        guard let lastRefreshed else { return }
        DiskStore.saveVersioned(
            Cache(conversations: conversations, lastRefreshed: lastRefreshed),
            name: Self.cacheName,
            version: Self.schemaVersion)
    }
}
