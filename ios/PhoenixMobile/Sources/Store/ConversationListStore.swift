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

    init() {
        conversations = DiskStore.load([Conversation].self, name: Self.cacheName) ?? []
        lastRefreshed = DiskStore.load(Meta.self, name: Self.metaName)?.lastRefreshed
    }

    func refresh(api: PhoenixAPI) async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        do {
            let fresh = try await api.listConversations()
            apply(fresh)
            lastError = nil
        } catch {
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
}
