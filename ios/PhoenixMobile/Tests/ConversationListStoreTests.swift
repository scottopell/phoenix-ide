import XCTest

@testable import PhoenixMobile

final class ConversationListStoreTests: XCTestCase {
    private func conversation(id: String, title: String) throws -> Conversation {
        let data = try JSONSerialization.data(withJSONObject: [
            "id": id,
            "slug": id,
            "title": title,
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
}
