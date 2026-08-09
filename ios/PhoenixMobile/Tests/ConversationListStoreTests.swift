import XCTest

@testable import PhoenixMobile

final class ConversationListStoreTests: XCTestCase {
    private func conversation(
        id: String,
        title: String,
        archived: Bool = false
    ) throws -> Conversation {
        let data = try JSONSerialization.data(withJSONObject: [
            "id": id,
            "slug": id,
            "title": title,
            "archived": archived,
        ])
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
    func testRefreshMergeCannotResurrectArchivedRows() throws {
        let merged = ConversationListStore.merging(
            [
                try conversation(id: "fresh-archived", title: "old", archived: true),
                try conversation(id: "removed-during-refresh", title: "old"),
                try conversation(id: "active", title: "active"),
            ],
            preserving: [
                "pushed-archived": try conversation(
                    id: "pushed-archived", title: "archived", archived: true),
                "removed-during-refresh": try conversation(
                    id: "removed-during-refresh", title: "newer update"),
            ],
            excluding: ["removed-during-refresh"])

        XCTAssertEqual(merged.map(\.id), ["active"])
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
