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

    func testCoordinatorIdentityComesFromServerRuntimeRole() throws {
        let conversation = try JSONDecoder().decode(
            Conversation.self,
            from: Data(
                #"{"id":"fleet","slug":"fleet","runtime_role":"coordinator"}"#.utf8))

        XCTAssertTrue(conversation.isCoordinator)
    }

    func testConversationAggregateIdentityFallsBackToTranscriptId() throws {
        let conversation = try JSONDecoder().decode(
            Conversation.self,
            from: Data(#"{"id":"legacy-conversation","slug":"legacy-conversation"}"#.utf8))

        XCTAssertEqual(conversation.aggregateIdentity, "legacy-conversation")
        XCTAssertEqual(conversation.transcriptRowIdentity, "legacy-conversation")
    }

    func testConversationAggregateIdentityUsesProductConversationIdWhenPresent() throws {
        let conversation = try JSONDecoder().decode(
            Conversation.self,
            from: Data(#"{"id":"row-2","product_conversation_id":"pc-2","slug":"root"}"#.utf8))

        XCTAssertEqual(conversation.aggregateIdentity, "pc-2")
        XCTAssertEqual(conversation.transcriptRowIdentity, "row-2")
    }

    func testProductConversationListRowDecodesStatePresentation() throws {
        let row = try JSONDecoder().decode(
            ProductConversationListRow.self,
            from: Data(#"{"product_conversation_id":"pc-123","canonical_route":"/product-conversations/pc-123","canonical_root":{"transcript_row_id":"root-123","slug":"root-slug","title":"Root title"},"ordinary_lifecycle":"open","latest_transcript_row_id":"latest-123","updated_at":"2025-01-02T03:04:05Z","presentation":{"kind":"state","display_name":"Root title","presentation_mode":"working"}}"#.utf8))

        XCTAssertEqual(row.product_conversation_id, "pc-123")
        XCTAssertEqual(row.canonical_route, "/product-conversations/pc-123")
        XCTAssertEqual(row.canonical_root.transcript_row_id, "root-123")
        XCTAssertEqual(row.ordinary_lifecycle, .open)
        XCTAssertEqual(row.latest_transcript_row_id, "latest-123")
        XCTAssertEqual(row.updated_at, "2025-01-02T03:04:05Z")
        XCTAssertEqual(
            row.presentation,
            .state(displayName: "Root title", presentationMode: "working"))
    }

    func testProductConversationListRowDecodesNeedsActionPresentation() throws {
        let row = try JSONDecoder().decode(
            ProductConversationListRow.self,
            from: Data(#"{"product_conversation_id":"pc-need","canonical_route":"/product-conversations/pc-need","canonical_root":{"transcript_row_id":"root-need","slug":"root-need","title":"Needs help"},"ordinary_lifecycle":"open","latest_transcript_row_id":"latest-need","updated_at":"2025-01-02T03:04:05Z","presentation":{"kind":"needs_action","display_name":"Needs help"}}"#.utf8))

        XCTAssertEqual(
            row.presentation,
            .needsAction(displayName: "Needs help"))
    }

    func testProductConversationSnapshotDecodesCoreIdentityAndSegments() throws {
        let snapshot = try JSONDecoder().decode(
            ProductConversationSnapshot.self,
            from: Data(#"{"product_conversation_id":"pc-1","close":null,"canonical_route":"/product-conversations/pc-1","requested_transcript_row_id":"latest-1","canonical_root":{"transcript_row_id":"root-1","slug":"root-1","title":"Root"},"ordinary_lifecycle":"open","latest_transcript_row_id":"latest-1","writable_transcript_row_id":"latest-1","updated_at":"2025-01-02T03:04:05Z","presentation":{"kind":"state","display_name":"Root","presentation_mode":"idle"},"work_identity":null,"source":null,"chain_qa_compatibility":null,"segments":[{"segment_ordinal":0,"transcript_row_id":"latest-1","slug":"root-1","title":"Root","messages":[],"handoff":null}],"before":null,"has_older":false}"#.utf8))

        XCTAssertEqual(snapshot.product_conversation_id, "pc-1")
        XCTAssertEqual(snapshot.canonical_route, "/product-conversations/pc-1")
        XCTAssertEqual(snapshot.requested_transcript_row_id, "latest-1")
        XCTAssertEqual(snapshot.canonical_root.transcript_row_id, "root-1")
        XCTAssertEqual(snapshot.ordinary_lifecycle, .open)
        XCTAssertEqual(snapshot.latest_transcript_row_id, "latest-1")
        XCTAssertEqual(snapshot.writable_transcript_row_id, "latest-1")
        XCTAssertEqual(snapshot.segments.count, 1)
        XCTAssertEqual(snapshot.segments[0].transcript_row_id, "latest-1")
        XCTAssertEqual(snapshot.presentation, .state(displayName: "Root", presentationMode: "idle"))
    }

    func testProductConversationSnapshotDecodesHistoricalHandoffAndSource() throws {
        let snapshot = try JSONDecoder().decode(
            ProductConversationSnapshot.self,
            from: Data(#"{"product_conversation_id":"pc-2","close":{"attempt_id":"close-1","phase":"awaiting_retirement_inspection","confirmation_snapshot":null,"inspections":[],"losses":[],"residuals":[]},"canonical_route":"/product-conversations/pc-2","requested_transcript_row_id":"latest-2","canonical_root":{"transcript_row_id":"root-2","slug":"root-2","title":"Root 2"},"ordinary_lifecycle":"history","latest_transcript_row_id":"latest-2","writable_transcript_row_id":null,"updated_at":"2025-01-02T03:04:05Z","presentation":{"kind":"needs_action","display_name":"Root 2"},"work_identity":null,"source":{"status":"deleted","source_product_conversation_id":"pc-source","source_conversation_id":"source-row","relation":"approved_task","relation_key":"task-123"},"chain_qa_compatibility":{"url":"/api/chains/root-2","root_transcript_row_id":"root-2"},"segments":[{"segment_ordinal":0,"transcript_row_id":"latest-2","slug":"root-2","title":"Root 2","messages":[],"handoff":{"kind":"historical","predecessor_transcript_row_id":"root-2","successor_transcript_row_id":"latest-2","continuation_message_id":"handoff-1","summary":"continued"}}],"before":"cursor-1","has_older":true}"#.utf8))

        XCTAssertEqual(snapshot.ordinary_lifecycle, .history)
        XCTAssertEqual(snapshot.close?.phase, .awaiting_retirement_inspection)
        XCTAssertEqual(snapshot.presentation, .needsAction(displayName: "Root 2"))
        XCTAssertEqual(
            snapshot.source,
            .deleted(
                sourceProductConversationId: "pc-source",
                sourceConversationId: "source-row",
                relation: .approved_task,
                relationKey: "task-123"))
        XCTAssertEqual(snapshot.chain_qa_compatibility?.root_transcript_row_id, "root-2")
        XCTAssertEqual(
            snapshot.segments.first?.handoff,
            .historical(
                predecessorTranscriptRowId: "root-2",
                successorTranscriptRowId: "latest-2",
                continuationMessageId: "handoff-1",
                summary: "continued"))
        XCTAssertEqual(snapshot.before, "cursor-1")
        XCTAssertTrue(snapshot.has_older)
    }

    func testTranscriptRowConversationCarriesDistinctAggregateIdentity() throws {
        let conversation = try JSONDecoder().decode(
            Conversation.self,
            from: Data(#"{"id":"row-2","product_conversation_id":"pc-2","slug":"root"}"#.utf8))

        XCTAssertEqual(conversation.aggregateIdentity, "pc-2")
        XCTAssertEqual(conversation.transcriptRowIdentity, "row-2")
    }
}
