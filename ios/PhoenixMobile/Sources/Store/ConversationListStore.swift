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
    private(set) var isRefreshing = false
    private(set) var lastError: String?

    private static let cacheName = "conversations"
    private static let metaName = "conversations-meta"

    private struct Meta: Codable {
        var lastRefreshed: Date
    }

    /// Bumped by reset(); a refresh started under an older generation
    /// discards its response, so an in-flight fetch from the previous
    /// server cannot repopulate a just-cleared cache.
    private var generation = 0

    init() {
        conversations = DiskStore.load([Conversation].self, name: Self.cacheName) ?? []
        lastRefreshed = DiskStore.load(Meta.self, name: Self.metaName)?.lastRefreshed
    }

    func refresh(api: PhoenixAPI) async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        let startedGeneration = generation
        do {
            let fresh = try await api.listConversations()
            guard generation == startedGeneration else { return }
            apply(fresh)
            lastError = nil
        } catch {
            guard generation == startedGeneration else { return }
            // Keep the cached list — stale data beats no data offline.
            lastError = (error as? APIError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func apply(_ fresh: [Conversation]) {
        conversations = fresh.sorted {
            ($0.updatedAtDate ?? .distantPast) > ($1.updatedAtDate ?? .distantPast)
        }
        lastRefreshed = Date()
        DiskStore.save(conversations, name: Self.cacheName)
        DiskStore.save(Meta(lastRefreshed: lastRefreshed!), name: Self.metaName)
    }

    /// Merge a single updated conversation (e.g. after creation or an SSE
    /// update in an open session) without waiting for a full refresh.
    func upsert(_ conversation: Conversation) {
        if let idx = conversations.firstIndex(where: { $0.id == conversation.id }) {
            conversations[idx] = conversation
        } else {
            conversations.insert(conversation, at: 0)
        }
        conversations.sort {
            ($0.updatedAtDate ?? .distantPast) > ($1.updatedAtDate ?? .distantPast)
        }
        DiskStore.save(conversations, name: Self.cacheName)
    }

    func remove(id: String) {
        conversations.removeAll { $0.id == id }
        DiskStore.save(conversations, name: Self.cacheName)
    }

    /// Drop in-memory state after the disk cache is cleared (or the user
    /// signs out). Without this the long-lived store keeps showing
    /// supposedly-deleted rows until a successful refresh. Also invalidates
    /// any in-flight refresh so its late response can't repopulate the
    /// cleared cache with the previous server's data.
    func reset() {
        generation += 1
        conversations = []
        lastRefreshed = nil
        lastError = nil
    }
}
