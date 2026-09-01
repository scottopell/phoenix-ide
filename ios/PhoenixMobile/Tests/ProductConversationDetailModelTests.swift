import XCTest

@testable import PhoenixMobile

@MainActor
final class ProductConversationDetailModelTests: XCTestCase {
    private func makeAPI() -> PhoenixAPI {
        PhoenixAPI(baseURL: URL(string: "https://example.com")!, password: nil, allowSelfSigned: true)!
    }

    private func makeSession(id: String) -> ConversationSession {
        ConversationSession(conversationId: id, api: makeAPI(), connectivity: ConnectivityMonitor())
    }

    private func snapshot(
        lifecycle: ProductConversationOrdinaryLifecycle = .open,
        latest: String = "row-2",
        writable: String? = "row-2"
    ) -> ProductConversationSnapshot {
        ProductConversationSnapshot(
            product_conversation_id: "pc-1",
            close: nil,
            canonical_route: "/product-conversations/pc-1",
            requested_transcript_row_id: latest,
            canonical_root: .init(transcript_row_id: "row-1", slug: "root", title: "Root"),
            ordinary_lifecycle: lifecycle,
            latest_transcript_row_id: latest,
            writable_transcript_row_id: writable,
            updated_at: "2025-01-02T03:04:05Z",
            presentation: .state(displayName: "Root", presentationMode: "working"),
            work_identity: nil,
            source: nil,
            chain_qa_compatibility: nil,
            segments: [
                .init(
                    segment_ordinal: 0,
                    transcript_row_id: "row-1",
                    slug: "root",
                    title: "Root",
                    messages: [
                        .init(
                            message_id: "m-1",
                            conversation_id: "row-1",
                            sequence_id: 4,
                            message_type: "user",
                            content: .object(["text": .string("before")]),
                            display_data: nil,
                            created_at: "2025-01-02T03:04:05Z")
                    ],
                    handoff: .historical(
                        predecessorTranscriptRowId: "row-1",
                        successorTranscriptRowId: "row-2",
                        continuationMessageId: "m-cont",
                        summary: "summary")),
                .init(
                    segment_ordinal: 1,
                    transcript_row_id: "row-2",
                    slug: "next",
                    title: "Next",
                    messages: [
                        .init(
                            message_id: "m-2",
                            conversation_id: "row-2",
                            sequence_id: 1,
                            message_type: "agent",
                            content: .object(["text": .string("after")]),
                            display_data: nil,
                            created_at: "2025-01-02T03:04:06Z")
                    ],
                    handoff: nil),
            ],
            before: nil,
            has_older: false)
    }

    func testPrefersWritableTranscriptSessionWithoutRetargetingExistingSessions() {
        let row1 = makeSession(id: "row-1")
        let row2 = makeSession(id: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in
                switch id {
                case "row-1": row1
                case "row-2": row2
                default: nil
                }
            })

        model.apply(snapshot: snapshot())

        XCTAssertTrue(model.actionSession === row2)
        XCTAssertEqual(model.actionTranscriptRowId, "row-2")
        XCTAssertEqual(row2.conversationId, "row-2")
    }

    func testRebindSelectsNewWritableTranscriptSession() async {
        let row1 = makeSession(id: "row-1")
        let row2 = makeSession(id: "row-2")
        let row3 = makeSession(id: "row-3")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in
                switch id {
                case "row-1": row1
                case "row-2": row2
                case "row-3": row3
                default: nil
                }
            })

        await model.start()
        model.apply(snapshot: snapshot())
        XCTAssertTrue(model.actionSession === row2)

        var rebound = snapshot(latest: "row-3", writable: "row-3")
        rebound.segments.append(
            .init(
                segment_ordinal: 2,
                transcript_row_id: "row-3",
                slug: "third",
                title: "Third",
                messages: [],
                handoff: nil))
        model.apply(snapshot: rebound)

        XCTAssertTrue(model.actionSession === row3)
        XCTAssertEqual(model.actionTranscriptRowId, "row-3")
    }

    func testHistoryIsReadOnlyAndUsesNoOutboxOwner() {
        let row2 = makeSession(id: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in id == "row-2" ? row2 : nil })

        model.apply(snapshot: snapshot(lifecycle: .history, writable: nil))

        XCTAssertTrue(model.isReadOnly)
        XCTAssertTrue(model.stateDetailSession === row2)
        XCTAssertTrue(model.actionSession === row2)
    }

    func testComposedMessagesPreserveSegmentBoundaryOrderWhenSequenceResets() {
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { _ in nil })

        model.apply(snapshot: snapshot())

        let items = model.transcriptItems
        XCTAssertEqual(items.count, 3)
        XCTAssertEqual(items.map(\.debugLabel), ["message:m-1", "handoff:summary", "message:m-2"])
        XCTAssertEqual(model.segments.map(\.transcript_row_id), ["row-1", "row-2"])
        XCTAssertEqual(model.segments[0].handoff?.summaryText, "summary")
    }

    func testSelectingHistoricalSegmentDoesNotMoveWritableDelegate() {
        let row1 = makeSession(id: "row-1")
        let row2 = makeSession(id: "row-2")
        let model = ProductConversationDetailModel(
            aggregateId: "pc-1",
            api: makeAPI(),
            connectivity: ConnectivityMonitor(),
            sessionProvider: { id in
                switch id {
                case "row-1": row1
                case "row-2": row2
                default: nil
                }
            })

        model.apply(snapshot: snapshot())
        model.selectTranscriptRow(id: "row-1")

        XCTAssertEqual(model.selectedTranscriptRowId, "row-1")
        XCTAssertEqual(model.actionTranscriptRowId, "row-2")
        XCTAssertTrue(model.actionSession === row2)
    }
}

private extension ProductConversationTranscriptItem {
    var debugLabel: String {
        switch self {
        case .message(let message):
            "message:\(message.message_id)"
        case .handoff(let handoff):
            "handoff:\(handoff.summaryText)"
        }
    }
}

private extension ProductConversationHandoff {
    var summaryText: String {
        switch self {
        case .completed(_, _, _, _, let summary):
            summary
        case .historical(_, _, _, let summary):
            summary
        }
    }
}
