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

    static let cacheName = "conversations"
    static let schemaVersion = 2

    struct CacheV1: Codable {
        var conversations: [Conversation]
        var lastRefreshed: Date
    }

    struct Cache: Codable {
        var conversations: [Conversation]
        var transcriptToAggregate: [String: String]
        var aggregateToCachedTranscript: [String: String]
        var lastRefreshed: Date
    }

    private struct SnapshotMetadata: Decodable {
        var conversation: Conversation?
    }

    private(set) var transcriptToAggregate: [String: String] = [:]
    private(set) var aggregateToCachedTranscript: [String: String] = [:]
    private let hasCachedSnapshot: (String) -> Bool
    private let cacheSource: URL
    private let cacheWriter: VersionedDiskWriter
    private let snapshotDirectory: URL

    /// Reset invalidates an in-flight refresh. Row-level changes are folded
    /// into its result so the refresh can still update unrelated rows.
    private var generation = 0
    private var refreshToken: UUID?
    /// Changes on every list mutation, including non-destructive upserts.
    /// Background fetches use this separately from foreground refresh merging.
    private var externalMutationGeneration = 0
    /// Server-pushed rows received after the current full refresh began,
    /// keyed by ProductConversation aggregate identity.
    private var upsertsDuringRefresh: [String: Conversation] = [:]
    /// Aggregate identities removed or archived after the current full refresh began.
    private var exclusionsDuringRefresh: Set<String> = []

    init(
        hasCachedSnapshot: @escaping (String) -> Bool,
        context: VersionedDiskContext
    ) {
        self.hasCachedSnapshot = hasCachedSnapshot
        self.cacheSource = context.rootDirectory.appendingPathComponent(Self.cacheName).appendingPathExtension("json")
        self.cacheWriter = context.writer(name: Self.cacheName, version: Self.schemaVersion)
        self.snapshotDirectory = context.rootDirectory
        if case .value(let cache) = DiskStore.loadVersionedResult(
            Cache.self, source: cacheSource, version: Self.schemaVersion)
        {
            conversations = Self.sortedByUpdatedAt(Self.merging(cache.conversations, preserving: [:]))
            lastRefreshed = cache.lastRefreshed
            rebuildIndexes(from: cache)
            return
        }
        if case .value(let legacy) = DiskStore.loadVersionedResult(
            CacheV1.self, source: cacheSource, version: 1)
        {
            conversations = Self.sortedByUpdatedAt(Self.merging(legacy.conversations, preserving: [:]))
            lastRefreshed = legacy.lastRefreshed
            rebuildIndexesFromConversations()
            hydrateIndexesFromSnapshots()
            persistCache()
        }
    }

    func refresh(api: PhoenixAPI) async -> [String: String] {
        guard refreshToken == nil else { return [:] }
        externalMutationGeneration += 1
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
            let response = try await api.listProductConversations()
            guard generation == startedGeneration else { return [:] }
            let latestByAggregate = Dictionary(uniqueKeysWithValues: response.product_conversations.map {
                ($0.product_conversation_id, $0.latest_transcript_row_id)
            })
            externalMutationGeneration += 1
            apply(Self.merging(
                response.product_conversations.map(api.productConversationListRowToConversation),
                preserving: upsertsDuringRefresh,
                excluding: exclusionsDuringRefresh))
            for (aggregateId, transcriptRowId) in latestByAggregate where aggregateExists(aggregateId) {
                transcriptToAggregate[transcriptRowId] = aggregateId
            }
            persistCache()
            lastError = nil
            return latestByAggregate
        } catch {
            guard generation == startedGeneration else { return [:] }
            lastError = (error as? APIError)?.errorDescription ?? error.localizedDescription
            return [:]
        }
    }

    nonisolated static func merging(
        _ fresh: [Conversation],
        preserving upserts: [String: Conversation],
        excluding exclusions: Set<String> = []
    ) -> [Conversation] {
        var byId = Dictionary(uniqueKeysWithValues: fresh
            .filter { $0.archived != true && !exclusions.contains($0.aggregateIdentity) }
            .map { ($0.aggregateIdentity, $0) })
        for (id, conversation) in upserts
        where conversation.archived != true && !exclusions.contains(id) {
            byId[id] = conversation
        }
        return Array(byId.values)
    }

    private func rebuildIndexes(from cache: Cache) {
        rebuildIndexesFromConversations()
        for (transcriptId, aggregateId) in cache.transcriptToAggregate where aggregateExists(aggregateId) {
            transcriptToAggregate[transcriptId] = aggregateId
        }
        for (aggregateId, transcriptId) in cache.aggregateToCachedTranscript where aggregateExists(aggregateId) {
            aggregateToCachedTranscript[aggregateId] = transcriptId
        }
    }

    private func rebuildIndexesFromConversations() {
        transcriptToAggregate.removeAll(keepingCapacity: true)
        aggregateToCachedTranscript.removeAll(keepingCapacity: true)
        for conversation in conversations {
            transcriptToAggregate[conversation.transcriptRowIdentity] = conversation.aggregateIdentity
            aggregateToCachedTranscript[conversation.aggregateIdentity] = conversation.transcriptRowIdentity
        }
    }

    private func aggregateExists(_ aggregateId: String) -> Bool {
        conversations.contains { $0.aggregateIdentity == aggregateId }
    }

    private func hydrateIndexesFromSnapshots() {
        let names = (try? FileManager.default.contentsOfDirectory(
            at: snapshotDirectory,
            includingPropertiesForKeys: nil))?
            .filter { $0.lastPathComponent.hasPrefix("conv-") && $0.pathExtension == "json" }
            .map { $0.deletingPathExtension().lastPathComponent } ?? []
        for name in names {

            guard case .value(let snapshot) = DiskStore.loadVersionedResult(
                SnapshotMetadata.self,
                source: cacheSource.deletingLastPathComponent().appendingPathComponent(name).appendingPathExtension("json"),
                version: 1),
                let conversation = snapshot.conversation,
                let aggregateId = conversation.product_conversation_id,
                aggregateExists(aggregateId)
            else { continue }
            transcriptToAggregate[conversation.transcriptRowIdentity] = aggregateId
            if hasCachedSnapshot(conversation.transcriptRowIdentity) {
                aggregateToCachedTranscript[aggregateId] = conversation.transcriptRowIdentity
            }
        }
    }

    private func mergeCanonicalMetadata(into fresh: [Conversation]) -> [Conversation] {
        let existingByAggregate = Dictionary(uniqueKeysWithValues: conversations.map {
            ($0.aggregateIdentity, $0)
        })
        return fresh.map { incoming in
            guard let existing = existingByAggregate[incoming.aggregateIdentity] else {
                return incoming
            }
            return Conversation(
                id: incoming.id,
                product_conversation_id: incoming.product_conversation_id,
                slug: incoming.slug,
                title: incoming.title,
                model: incoming.model,
                cwd: incoming.cwd,
                created_at: incoming.created_at,
                updated_at: incoming.updated_at,
                message_count: incoming.message_count,
                state: incoming.state,
                state_updated_at: incoming.state_updated_at,
                branch_name: incoming.branch_name,
                task_title: existing.task_title,
                archived: incoming.archived,
                project_name: incoming.project_name,
                conv_mode_label: incoming.conv_mode_label,
                presentation_mode: incoming.presentation_mode,
                requires_action: incoming.requires_action,
                transcript_generation: incoming.transcript_generation,
                runtime_role: incoming.runtime_role)
        }
    }

    private func apply(_ fresh: [Conversation]) {
        let priorAliases = transcriptToAggregate
        let priorCached = aggregateToCachedTranscript
        conversations = Self.sortedByUpdatedAt(mergeCanonicalMetadata(into: fresh))
        rebuildIndexesFromConversations()
        for (transcriptId, aggregateId) in priorAliases where aggregateExists(aggregateId) {
            transcriptToAggregate[transcriptId] = aggregateId
        }
        for (aggregateId, cachedTranscriptId) in priorCached where aggregateExists(aggregateId) {
            if hasCachedSnapshot(cachedTranscriptId) {
                aggregateToCachedTranscript[aggregateId] = cachedTranscriptId
            }
        }
        lastRefreshed = Date()
        persistCache()
    }

    /// Apply an externally fetched list (background attention check) so a
    /// cold open renders fresher data. Skipped while a foreground refresh
    /// is in flight; the generation guard semantics match refresh().
    struct ExternalRefreshToken: Equatable {
        fileprivate let generation: Int
    }

    func externalRefreshToken() -> ExternalRefreshToken {
        ExternalRefreshToken(generation: externalMutationGeneration)
    }

    func canApplyExternal(startedAt token: ExternalRefreshToken) -> Bool {
        !isRefreshing && token.generation == externalMutationGeneration
    }

    @discardableResult
    func applyExternal(_ fresh: [Conversation], startedAt token: ExternalRefreshToken) -> Bool {
        guard canApplyExternal(startedAt: token) else { return false }
        apply(Self.merging(fresh, preserving: [:]))
        lastError = nil
        return true
    }

    /// Merge a single updated conversation (e.g. after creation or an SSE
    /// update in an open session) without waiting for a full refresh.
    func registerTranscriptAlias(
        transcriptRowId: String,
        aggregateId: String
    ) {
        guard aggregateExists(aggregateId) else { return }
        transcriptToAggregate[transcriptRowId] = aggregateId
        if hasCachedSnapshot(transcriptRowId) {
            aggregateToCachedTranscript[aggregateId] = transcriptRowId
        }
        persistCache()
    }

    func upsert(_ conversation: Conversation) {
        if lastRefreshed == nil { lastRefreshed = Date() }
        externalMutationGeneration += 1
        let aggregateIdentity = conversation.aggregateIdentity
        if conversation.archived == true {
            if isRefreshing {
                upsertsDuringRefresh[aggregateIdentity] = nil
                exclusionsDuringRefresh.insert(aggregateIdentity)
            }
            conversations.removeAll { $0.aggregateIdentity == aggregateIdentity }
            transcriptToAggregate = transcriptToAggregate.filter { $0.value != aggregateIdentity }
            aggregateToCachedTranscript[aggregateIdentity] = nil
            persistCache()
            return
        }
        if isRefreshing {
            exclusionsDuringRefresh.remove(aggregateIdentity)
            upsertsDuringRefresh[aggregateIdentity] = conversation
        }
        transcriptToAggregate[conversation.transcriptRowIdentity] = aggregateIdentity
        if hasCachedSnapshot(conversation.transcriptRowIdentity)
            || aggregateToCachedTranscript[aggregateIdentity] == nil
        {
            aggregateToCachedTranscript[aggregateIdentity] = conversation.transcriptRowIdentity
        }
        if let idx = conversations.firstIndex(where: { $0.aggregateIdentity == aggregateIdentity }) {
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

    func remove(aggregateId: String) {
        removeFromMemory(aggregateId: aggregateId)
        persistCache()
    }

    func removeAndPersist(aggregateId: String) async -> Bool {
        removeFromMemory(aggregateId: aggregateId)
        guard let lastRefreshed else { return true }
        let revision = cacheWriter.reserveRevision()
        return await cacheWriter.save(
            Cache(
                conversations: conversations,
                transcriptToAggregate: transcriptToAggregate,
                aggregateToCachedTranscript: aggregateToCachedTranscript,
                lastRefreshed: lastRefreshed),
            revision: revision)
    }

    private func removeFromMemory(aggregateId: String) {
        externalMutationGeneration += 1
        upsertsDuringRefresh[aggregateId] = nil
        if isRefreshing {
            exclusionsDuringRefresh.insert(aggregateId)
        }
        conversations.removeAll { $0.aggregateIdentity == aggregateId }
        transcriptToAggregate = transcriptToAggregate.filter { $0.value != aggregateId }
        aggregateToCachedTranscript[aggregateId] = nil
    }

    func aggregateId(forTranscriptRowId transcriptRowId: String) -> String? {
        transcriptToAggregate[transcriptRowId]
    }

    func cachedTranscriptRowId(forAggregateId aggregateId: String) -> String? {
        aggregateToCachedTranscript[aggregateId]
    }

    func cachedNavigationTranscriptRowId(forAggregateId aggregateId: String, latestTranscriptRowId: String) -> String {
        if hasCachedSnapshot(latestTranscriptRowId) {
            return latestTranscriptRowId
        }
        if let cached = aggregateToCachedTranscript[aggregateId],
           hasCachedSnapshot(cached)
        {
            return cached
        }
        for (transcriptId, mappedAggregateId) in transcriptToAggregate where mappedAggregateId == aggregateId {
            if hasCachedSnapshot(transcriptId) {
                return transcriptId
            }
        }
        return latestTranscriptRowId
    }

    func removeByTranscriptRowId(_ transcriptRowId: String) {
        guard let aggregateId = aggregateId(forTranscriptRowId: transcriptRowId) else {
            return
        }
        remove(aggregateId: aggregateId)
    }

    func reset() async {
        let revision = cacheWriter.reserveRevision()
        generation += 1
        refreshToken = nil
        externalMutationGeneration += 1
        upsertsDuringRefresh.removeAll()
        exclusionsDuringRefresh.removeAll()
        conversations = []
        transcriptToAggregate = [:]
        aggregateToCachedTranscript = [:]
        lastRefreshed = nil
        lastError = nil
        await cacheWriter.remove(revision: revision)
    }

    private func persistCache() {
        guard let lastRefreshed else { return }
        let revision = cacheWriter.reserveRevision()
        Task { await cacheWriter.save(
            Cache(
                conversations: conversations,
                transcriptToAggregate: transcriptToAggregate,
                aggregateToCachedTranscript: aggregateToCachedTranscript,
                lastRefreshed: lastRefreshed),
            revision: revision)
        }
    }
}
