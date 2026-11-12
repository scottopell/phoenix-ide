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

    func testDecodesCachedPr() throws {
        let pr = try decode(
            ProductConversationPr.self,
            #"{"number":98,"title":"Merged","url":"https://example.invalid/98","display_state":"merged","base":"main","head":"x","feedback_status":"in_progress"}"#)
        XCTAssertEqual(pr.number, 98)
        XCTAssertEqual(pr.display_state, .merged)
        XCTAssertEqual(pr.feedback_status, .inProgress)
    }

    func testPrBadgeLabels() {
        var pr = ProductConversationPr(
            number: 123, title: "t", url: "u", display_state: .open,
            base: "main", head: "h", feedback_status: .open)
        XCTAssertEqual(PrBadge.label(for: pr), "#123")
        XCTAssertEqual(PrBadge.accessibilityLabel(for: pr), "PR 123")
        pr.display_state = .merged
        pr.feedback_status = .approved
        XCTAssertEqual(PrBadge.label(for: pr), "#123 merged")
        XCTAssertEqual(PrBadge.accessibilityLabel(for: pr), "PR 123, merged, feedback approved")
        pr.display_state = .draft
        XCTAssertEqual(PrBadge.label(for: pr), "#123 draft")
    }
}
