import Foundation
import XCTest

@testable import PhoenixMobile

final class ConversationListStoreTests: XCTestCase {
    private struct LegacyCache: Codable {
        var conversations: [Conversation]
        var lastRefreshed: Date
    }

    private struct PersistedCacheFixture: Codable {
        var conversations: [Conversation]
        var transcriptToAggregate: [String: String]
        var aggregateToCachedTranscript: [String: String]
        var lastRefreshed: Date
    }

    @MainActor
    private func makeContext(baseDirectory: URL) -> VersionedDiskContext {
        DiskStore.versionedContext(baseDirectory: baseDirectory)
    }

    @MainActor
    private func makeStore(baseDirectory: URL, hasCachedSnapshot: @escaping (String) -> Bool = { _ in false }) -> ConversationListStore {
        ConversationListStore(hasCachedSnapshot: hasCachedSnapshot, context: makeContext(baseDirectory: baseDirectory))
    }


    private func conversation(
        id: String,
        aggregateId: String? = nil,
        title: String,
        archived: Bool = false
    ) throws -> Conversation {
        var json: [String: Any] = [
            "id": id,
            "slug": id,
            "title": title,
            "archived": archived,
        ]
        json["product_conversation_id"] = aggregateId
        let data = try JSONSerialization.data(withJSONObject: json)
        return try JSONDecoder().decode(Conversation.self, from: data)
    }

    func testRefreshMergePreservesOnlyRowsUpsertedDuringRequest() throws {
        let merged = ConversationListStore.merging(
            [
                try conversation(id: "one", title: "stale"),
                try conversation(id: "two", title: "fresh"),
            ],
            preserving: [
                "one": try conversation(id: "one", title: "SSE update"),
                "three": try conversation(id: "three", title: "new conversation"),
            ])
        let byId = Dictionary(uniqueKeysWithValues: merged.map { ($0.id, $0) })

        XCTAssertEqual(byId["one"]?.title, "SSE update")
        XCTAssertEqual(byId["two"]?.title, "fresh")
        XCTAssertEqual(byId["three"]?.title, "new conversation")
    }

    func testRefreshMergeKeysRowsByAggregateIdentity() throws {
        let merged = ConversationListStore.merging(
            [
                try conversation(id: "root-a", aggregateId: "pc-1", title: "root"),
                try conversation(id: "other", aggregateId: "pc-2", title: "other"),
            ],
            preserving: [
                "pc-1": try conversation(id: "latest-a", aggregateId: "pc-1", title: "latest")
            ])
        let byAggregate = Dictionary(uniqueKeysWithValues: merged.map { ($0.aggregateIdentity, $0) })

        XCTAssertEqual(merged.count, 2)
        XCTAssertEqual(byAggregate["pc-1"]?.id, "latest-a")
        XCTAssertEqual(byAggregate["pc-1"]?.title, "latest")
        XCTAssertEqual(byAggregate["pc-2"]?.id, "other")
    }
    func testRefreshMergeCannotResurrectArchivedRows() throws {
        let merged = ConversationListStore.merging(
            [
                try conversation(id: "fresh-archived", aggregateId: "pc-archived", title: "old", archived: true),
                try conversation(id: "removed-during-refresh", aggregateId: "pc-removed", title: "old"),
                try conversation(id: "active", aggregateId: "pc-active", title: "active"),
            ],
            preserving: [
                "pushed-archived": try conversation(
                    id: "pushed-archived", aggregateId: "pc-pushed-archived", title: "archived", archived: true),
                "pc-removed": try conversation(
                    id: "removed-during-refresh", aggregateId: "pc-removed", title: "newer update"),
            ],
            excluding: ["pc-removed"])

        XCTAssertEqual(merged.map(\.aggregateIdentity), ["pc-active"])
    }

    @MainActor
    func testExternalRefreshCannotOverwriteAnInterveningUpsert() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = makeStore(baseDirectory: DiskStore.baseDirectory)
        let token = store.externalRefreshToken()
        store.upsert(try conversation(id: "one", title: "SSE update"))

        XCTAssertFalse(store.applyExternal(
            [try conversation(id: "one", title: "stale response")], startedAt: token))
        XCTAssertEqual(store.conversations.first?.title, "SSE update")
    }

    @MainActor
    func testRemoveByTranscriptRowIdRemovesAggregateRow() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = makeStore(baseDirectory: DiskStore.baseDirectory)
        store.upsert(try conversation(id: "latest-a", aggregateId: "pc-1", title: "latest"))

        store.removeByTranscriptRowId("latest-a")

        XCTAssertTrue(store.conversations.isEmpty)
    }

    @MainActor
    func testBackgroundExternalRefreshFiltersHistoryRows() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = makeStore(baseDirectory: DiskStore.baseDirectory)
        let token = store.externalRefreshToken()

        XCTAssertTrue(store.applyExternal([
            try conversation(id: "latest-open", aggregateId: "pc-open", title: "open"),
            try conversation(id: "latest-history", aggregateId: "pc-history", title: "history", archived: true),
        ], startedAt: token))
        XCTAssertEqual(store.conversations.map(\.aggregateIdentity), ["pc-open"])
    }

    @MainActor
    func testLegacyCacheRefreshThenTranscriptUpdateKeepsSingleAggregateProjection() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = makeStore(baseDirectory: DiskStore.baseDirectory)
        store.upsert(try conversation(id: "legacy-row", title: "legacy"))
        let refreshToken = store.externalRefreshToken()
        XCTAssertTrue(store.applyExternal([
            try conversation(id: "latest-row", aggregateId: "pc-1", title: "root title")
        ], startedAt: refreshToken))
        store.upsert(try conversation(id: "newer-row", aggregateId: "pc-1", title: "root title"))

        XCTAssertEqual(store.conversations.count, 1)
        XCTAssertEqual(store.conversations.first?.aggregateIdentity, "pc-1")
        XCTAssertEqual(store.conversations.first?.id, "newer-row")
    }

    @MainActor
    func testTranscriptAliasPersistsAcrossLatestTranscriptRotation() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = makeStore(baseDirectory: DiskStore.baseDirectory)
        store.upsert(try conversation(id: "row-1", aggregateId: "pc-1", title: "root"))
        store.upsert(try conversation(id: "row-2", aggregateId: "pc-1", title: "root"))

        XCTAssertEqual(store.aggregateId(forTranscriptRowId: "row-1"), "pc-1")
        XCTAssertEqual(store.aggregateId(forTranscriptRowId: "row-2"), "pc-1")
        XCTAssertEqual(store.conversations.count, 1)
        XCTAssertEqual(store.conversations.first?.id, "row-2")
    }

    @MainActor
    func testTranscriptAliasPersistsAcrossFullRefreshReplacement() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = makeStore(baseDirectory: DiskStore.baseDirectory)
        store.upsert(try conversation(id: "row-1", aggregateId: "pc-1", title: "predecessor"))
        let token = store.externalRefreshToken()

        XCTAssertTrue(store.applyExternal([
            try conversation(id: "row-2", aggregateId: "pc-1", title: "successor")
        ], startedAt: token))

        XCTAssertEqual(store.aggregateId(forTranscriptRowId: "row-1"), "pc-1")
        XCTAssertEqual(store.aggregateId(forTranscriptRowId: "row-2"), "pc-1")
        store.upsert(try conversation(id: "row-1", aggregateId: "pc-1", title: "late predecessor update"))

        XCTAssertEqual(store.conversations.count, 1)
        XCTAssertEqual(store.conversations.first?.aggregateIdentity, "pc-1")
    }

    @MainActor
    func testTranscriptAliasPersistsAcrossColdRestartAfterReplacement() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let first = makeStore(baseDirectory: DiskStore.baseDirectory)
        let predecessor = try conversation(id: "row-1", aggregateId: "pc-1", title: "predecessor")
        let successor = try conversation(id: "row-2", aggregateId: "pc-1", title: "successor")
        let writer = makeContext(baseDirectory: DiskStore.baseDirectory).writer(name: ConversationListStore.cacheName, version: ConversationListStore.schemaVersion)
        _ = await writer.save(
            PersistedCacheFixture(
                conversations: [successor],
                transcriptToAggregate: ["row-1": "pc-1", "row-2": "pc-1"],
                aggregateToCachedTranscript: ["pc-1": "row-2"],
                lastRefreshed: Date()),
            revision: writer.reserveRevision())

        let reloaded = makeStore(baseDirectory: DiskStore.baseDirectory)

        XCTAssertEqual(reloaded.aggregateId(forTranscriptRowId: predecessor.id), "pc-1")
        XCTAssertEqual(reloaded.aggregateId(forTranscriptRowId: successor.id), "pc-1")
        XCTAssertEqual(reloaded.cachedTranscriptRowId(forAggregateId: "pc-1"), successor.id)
        reloaded.upsert(try conversation(id: predecessor.id, aggregateId: "pc-1", title: "late predecessor update"))
        XCTAssertEqual(reloaded.conversations.count, 1)
        XCTAssertEqual(reloaded.conversations.first?.aggregateIdentity, "pc-1")
    }

    @MainActor
    func testFullRefreshPreservesCanonicalTaskTitleAcrossRestart() async throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let seed = try JSONSerialization.data(withJSONObject: [
            "id": "row-1",
            "product_conversation_id": "pc-1",
            "slug": "root",
            "title": "Canonical",
            "task_title": "Canonical Task",
        ])
        let seededConversation = try JSONDecoder().decode(Conversation.self, from: seed)
        let legacyWriter = makeContext(baseDirectory: DiskStore.baseDirectory).writer(name: ConversationListStore.cacheName, version: 1)
        _ = await legacyWriter.save(
            ConversationListStore.CacheV1(conversations: [seededConversation], lastRefreshed: Date()),
            revision: legacyWriter.reserveRevision())

        let first = makeStore(baseDirectory: DiskStore.baseDirectory)
        let token = first.externalRefreshToken()
        XCTAssertTrue(first.applyExternal([
            try conversation(id: "row-2", aggregateId: "pc-1", title: "successor")
        ], startedAt: token))

        let reloaded = makeStore(baseDirectory: DiskStore.baseDirectory)

        XCTAssertEqual(reloaded.conversations.first?.task_title, "Canonical Task")
    }


    @MainActor
    func testExternalRefreshAppliesWithoutAnInterveningMutation() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = makeStore(baseDirectory: DiskStore.baseDirectory)
        let token = store.externalRefreshToken()

        XCTAssertTrue(store.applyExternal(
            [try conversation(id: "one", title: "fresh")], startedAt: token))
        XCTAssertEqual(store.conversations.first?.title, "fresh")
    }

    @MainActor
    func testContextOwnedResetRemovesSameBaseCacheWithoutTouchingOtherBaseAndFencesLateSave() async throws {
        let baseA = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-reset-a-\(UUID().uuidString)", isDirectory: true)
        let baseB = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-reset-b-\(UUID().uuidString)", isDirectory: true)
        let contextA = makeContext(baseDirectory: baseA)
        let writerA = contextA.writer(name: ConversationListStore.cacheName, version: ConversationListStore.schemaVersion)
        let contextB = makeContext(baseDirectory: baseB)
        let writerB = contextB.writer(name: ConversationListStore.cacheName, version: ConversationListStore.schemaVersion)

        let storeA = ConversationListStore(hasCachedSnapshot: { _ in false }, context: contextA)
        let rowA = try conversation(id: "row-a", title: "late")
        storeA.upsert(rowA)
        let staleRevision = writerA.reserveRevision()

        let rowB = try conversation(id: "row-b", title: "other")
        _ = await writerB.save(
            PersistedCacheFixture(
                conversations: [rowB],
                transcriptToAggregate: [:],
                aggregateToCachedTranscript: [:],
                lastRefreshed: Date()),
            revision: writerB.reserveRevision())
        let reloadedBHot = ConversationListStore(hasCachedSnapshot: { _ in false }, context: contextB)
        XCTAssertEqual(reloadedBHot.conversations.map(\.id), [rowB.id])

        await storeA.reset()
        _ = await writerA.save(
            PersistedCacheFixture(
                conversations: [rowA],
                transcriptToAggregate: [:],
                aggregateToCachedTranscript: [:],
                lastRefreshed: Date()),
            revision: staleRevision)

        let reloadedA = ConversationListStore(hasCachedSnapshot: { _ in false }, context: contextA)
        let reloadedB = ConversationListStore(hasCachedSnapshot: { _ in false }, context: contextB)

        XCTAssertTrue(reloadedA.conversations.isEmpty)
        XCTAssertEqual(reloadedB.conversations.map(\.id), [rowB.id])
    }
}
