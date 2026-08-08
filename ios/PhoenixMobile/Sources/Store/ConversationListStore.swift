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

    private struct Cache: Codable {
        var conversations: [Conversation]
        var lastRefreshed: Date
    }

    /// Bumped by every local mutation (reset, upsert, remove); a refresh
    /// started under an older generation discards its response. This keeps
    /// an in-flight fetch from repopulating a cleared cache, resurrecting a
    /// just-archived row, or dropping a just-created one — the discarded
    /// refresh's data is strictly older than the mutation it would clobber.
    private var generation = 0
    private var refreshToken: UUID?
    /// Server-pushed rows received after the current full refresh began.
    private var upsertsDuringRefresh: [String: Conversation] = [:]

    init() {
        if let cache = DiskStore.load(Cache.self, name: Self.cacheName) {
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
            }
        }
        let startedGeneration = generation
        do {
            let fresh = try await api.listConversations()
            guard generation == startedGeneration else { return }
            apply(Self.merging(fresh, preserving: upsertsDuringRefresh))
            lastError = nil
        } catch {
            guard generation == startedGeneration else { return }
            // Keep the cached list — stale data beats no data offline.
            lastError = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    nonisolated static func merging(
        _ fresh: [Conversation],
        preserving upserts: [String: Conversation]
    ) -> [Conversation] {
        var byId = Dictionary(uniqueKeysWithValues: fresh.map { ($0.id, $0) })
        for (id, conversation) in upserts {
            byId[id] = conversation
        }
        return Array(byId.values)
    }

    private func apply(_ fresh: [Conversation]) {
        conversations = Self.sortedByUpdatedAt(fresh)
        lastRefreshed = Date()
        persistCache()
    }

    /// Merge a single updated conversation (e.g. after creation or an SSE
    /// update in an open session) without waiting for a full refresh.
    func upsert(_ conversation: Conversation) {
        if lastRefreshed == nil { lastRefreshed = Date() }
        if isRefreshing {
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
        generation += 1
        upsertsDuringRefresh[id] = nil
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
        conversations = []
        lastRefreshed = nil
        lastError = nil
    }

    private func persistCache() {
        guard let lastRefreshed else { return }
        DiskStore.save(
            Cache(conversations: conversations, lastRefreshed: lastRefreshed),
            name: Self.cacheName)
    }
}
