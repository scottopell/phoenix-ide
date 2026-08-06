import XCTest

@testable import PhoenixMobile

final class ModelCompatibilityTests: XCTestCase {
    func testNullConversationSlugFallsBackToIdentity() throws {
        let conversation = try JSONDecoder().decode(
            Conversation.self,
            from: Data("{\"id\":\"legacy-conversation\",\"slug\":null}".utf8))

        XCTAssertNil(conversation.slug)
        XCTAssertEqual(conversation.displayTitle, "legacy-conversation")
        XCTAssertEqual(conversation.displaySlug, "legacy-conversation")
    }
}
