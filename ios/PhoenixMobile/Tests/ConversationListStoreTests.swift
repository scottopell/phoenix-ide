import XCTest

@testable import PhoenixMobile

final class ConversationListStoreTests: XCTestCase {
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
        let store = ConversationListStore()
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
        let store = ConversationListStore()
        store.upsert(try conversation(id: "latest-a", aggregateId: "pc-1", title: "latest"))

        store.removeByTranscriptRowId("latest-a")

        XCTAssertTrue(store.conversations.isEmpty)
    }

    @MainActor
    func testBackgroundExternalRefreshFiltersHistoryRows() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = ConversationListStore()
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
        let store = ConversationListStore()
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
        let store = ConversationListStore()
        store.upsert(try conversation(id: "row-1", aggregateId: "pc-1", title: "root"))
        store.upsert(try conversation(id: "row-2", aggregateId: "pc-1", title: "root"))

        XCTAssertEqual(store.aggregateId(forTranscriptRowId: "row-1"), "pc-1")
        XCTAssertEqual(store.aggregateId(forTranscriptRowId: "row-2"), "pc-1")
        XCTAssertEqual(store.conversations.count, 1)
        XCTAssertEqual(store.conversations.first?.id, "row-2")
    }

    @MainActor
    func testExternalRefreshAppliesWithoutAnInterveningMutation() throws {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-list-tests-\(UUID().uuidString)")
        let store = ConversationListStore()
        let token = store.externalRefreshToken()

        XCTAssertTrue(store.applyExternal(
            [try conversation(id: "one", title: "fresh")], startedAt: token))
        XCTAssertEqual(store.conversations.first?.title, "fresh")
    }
}

