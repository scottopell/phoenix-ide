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
    /// Bump when the persisted list/meta shapes change incompatibly
    /// (DiskStore versioning rule).
    private static let schemaVersion = 1

    private struct Meta: Codable {
        var lastRefreshed: Date
    }

    /// Bumped by every local mutation (reset, upsert, remove); a refresh
    /// started under an older generation discards its response. This keeps
    /// an in-flight fetch from repopulating a cleared cache, resurrecting a
    /// just-archived row, or dropping a just-created one — the discarded
    /// refresh's data is strictly older than the mutation it would clobber.
    private var generation = 0

    init() {
        conversations = DiskStore.loadVersioned(
            [Conversation].self, name: Self.cacheName, version: Self.schemaVersion) ?? []
        lastRefreshed = DiskStore.loadVersioned(
            Meta.self, name: Self.metaName, version: Self.schemaVersion)?.lastRefreshed
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
        DiskStore.saveVersioned(conversations, name: Self.cacheName, version: Self.schemaVersion)
        DiskStore.saveVersioned(
            Meta(lastRefreshed: lastRefreshed!), name: Self.metaName,
            version: Self.schemaVersion)
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
        generation += 1
        if let idx = conversations.firstIndex(where: { $0.id == conversation.id }) {
            conversations[idx] = conversation
        } else {
            conversations.insert(conversation, at: 0)
        }
        conversations.sort {
            ($0.updatedAtDate ?? .distantPast) > ($1.updatedAtDate ?? .distantPast)
        }
        DiskStore.saveVersioned(conversations, name: Self.cacheName, version: Self.schemaVersion)
    }

    func remove(id: String) {
        generation += 1
        conversations.removeAll { $0.id == id }
        DiskStore.saveVersioned(conversations, name: Self.cacheName, version: Self.schemaVersion)
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
