import XCTest

@testable import PhoenixMobile

// Wire contract for ProductConversationReadStateView / CoordinatorListEntryView
// (ui/src/generated/ProductConversationReadStateView.ts).
final class ProductConversationReadStateTests: XCTestCase {

    private func decode<T: Decodable>(_ type: T.Type, _ raw: String) throws -> T {
        try JSONDecoder().decode(type, from: Data(raw.utf8))
    }

    func testDecodesReadAndUnreadReasons() throws {
        XCTAssertEqual(try decode(ProductConversationReadState.self, #"{"state":"read"}"#), .read)
        XCTAssertEqual(
            try decode(ProductConversationReadState.self, #"{"state":"unread","reason":"marked"}"#),
            .unread(.marked))
        XCTAssertEqual(
            try decode(ProductConversationReadState.self, #"{"state":"unread","reason":"new_activity"}"#),
            .unread(.newActivity))
    }

    func testRejectsUnknownState() {
        XCTAssertThrowsError(try decode(ProductConversationReadState.self, #"{"state":"archived"}"#))
    }

    func testRoundTripsThroughEncoding() throws {
        for state in [ProductConversationReadState.read, .unread(.marked), .unread(.newActivity)] {
            let data = try JSONEncoder().encode(state)
            XCTAssertEqual(try JSONDecoder().decode(ProductConversationReadState.self, from: data), state)
        }
    }

    func testDecodesCoordinatorEntry() throws {
        let entry = try decode(
            CoordinatorListEntry.self,
            #"{"product_conversation_id":"global","read_state":{"state":"unread","reason":"new_activity"}}"#)
        XCTAssertEqual(entry.product_conversation_id, "global")
        XCTAssertEqual(entry.read_state.unreadReason, .newActivity)
    }

    @MainActor
    func testListStoreRoutesCoordinatorReadStateSeparately() {
        let store = ConversationListStore()
        store.reset()
        XCTAssertEqual(store.readState(forAggregateId: "product"), .read)
        store.setReadState(.unread(.marked), forAggregateId: "product")
        XCTAssertEqual(store.readState(forAggregateId: "product"), .unread(.marked))
    }

    @MainActor
    func testExternalPageKeepsReadChangesMadeAfterItWasFetched() {
        let store = ConversationListStore()
        store.reset()
        let token = store.externalRefreshToken()
        store.setReadState(.unread(.marked), forAggregateId: "changed")
        let page = PhoenixAPI.ConversationListPage(
            conversations: [],
            readStates: ["changed": .read, "other": .unread(.newActivity)],
            coordinator: nil)
        XCTAssertTrue(store.applyExternal([], annotations: page, startedAt: token))
        XCTAssertEqual(store.readState(forAggregateId: "changed"), .unread(.marked))
        XCTAssertEqual(store.readState(forAggregateId: "other"), .unread(.newActivity))
    }
}
