import XCTest

@testable import PhoenixMobile

// Contract tests for attention-nudge transitions (REQ-IOS-018).
final class AttentionDiffTests: XCTestCase {

    private func conv(
        _ id: String,
        aggregateId: String? = nil,
        mode: String?,
        title: String = "t",
        archived: Bool? = nil
    ) -> Conversation {
        Conversation(
            id: id,
            product_conversation_id: aggregateId,
            slug: id,
            title: nil,
            model: nil,
            cwd: nil,
            created_at: nil,
            updated_at: nil,
            message_count: nil,
            state: nil,
            state_updated_at: nil,
            branch_name: nil,
            task_title: title,
            archived: archived,
            project_name: nil,
            conv_mode_label: nil,
            presentation_mode: mode,
            requires_action: nil,
            transcript_generation: nil,
            runtime_role: nil)
    }

    private func entry(_ mode: String) -> AttentionMonitor.Entry {
        AttentionMonitor.Entry(mode: mode, title: "t")
    }

    @MainActor
    func testEnteringNeedsActionNotifies() {
        let events = AttentionMonitor.diff(
            previous: ["c1": entry("working")],
            current: [conv("c1", mode: "needs_action")])
        XCTAssertEqual(events, [.needsAction(aggregateId: "c1", transcriptRowId: "c1", title: "t")])
    }

    @MainActor
    func testPersistingNeedsActionDoesNotRenotify() {
        let events = AttentionMonitor.diff(
            previous: ["c1": entry("needs_action")],
            current: [conv("c1", mode: "needs_action")])
        XCTAssertTrue(events.isEmpty)
    }

    @MainActor
    func testEnteringErrorNotifies() {
        let events = AttentionMonitor.diff(
            previous: ["c1": entry("working")],
            current: [conv("c1", mode: "error")])
        XCTAssertEqual(events, [.errored(aggregateId: "c1", transcriptRowId: "c1", title: "t")])
    }

    @MainActor
    func testWorkingToIdleOrDoneNotifiesFinished() {
        XCTAssertEqual(
            AttentionMonitor.diff(
                previous: ["c1": entry("working")],
                current: [conv("c1", mode: "idle")]),
            [.finished(aggregateId: "c1", transcriptRowId: "c1", title: "t")])
        XCTAssertEqual(
            AttentionMonitor.diff(
                previous: ["c1": entry("working")],
                current: [conv("c1", mode: "done")]),
            [.finished(aggregateId: "c1", transcriptRowId: "c1", title: "t")])
    }

    @MainActor
    func testIdleToDoneIsNotAFinish() {
        // Only a turn the user was plausibly waiting on (working) counts.
        let events = AttentionMonitor.diff(
            previous: ["c1": entry("idle")],
            current: [conv("c1", mode: "done")])
        XCTAssertTrue(events.isEmpty)
    }

    @MainActor
    func testUnknownConversationSeedsSilently() {
        // First sight of a conversation must never notify — this is what
        // prevents a nudge burst on first run and fresh installs.
        let events = AttentionMonitor.diff(
            previous: [:],
            current: [conv("c1", mode: "needs_action"), conv("c2", mode: "error")])
        XCTAssertTrue(events.isEmpty)
    }

    @MainActor
    func testMultipleTransitionsProduceOneEventEach() {
        let events = AttentionMonitor.diff(
            previous: [
                "c1": entry("working"),
                "c2": entry("working"),
                "c3": entry("idle"),
            ],
            current: [
                conv("c1", mode: "needs_action"),
                conv("c2", mode: "idle"),
                conv("c3", mode: "idle"),
            ])
        XCTAssertEqual(events.count, 2)
        XCTAssertTrue(events.contains(.needsAction(aggregateId: "c1", transcriptRowId: "c1", title: "t")))
        XCTAssertTrue(events.contains(.finished(aggregateId: "c2", transcriptRowId: "c2", title: "t")))
    }

    @MainActor
    func testEntriesSnapshotUsesPresentationModeAndTitle() {
        let entries = AttentionMonitor.entries(
            from: [conv("c1", mode: "working", title: "fix login")])
        XCTAssertEqual(
            entries, ["c1": AttentionMonitor.Entry(mode: "working", title: "fix login")])
    }

    @MainActor
    func testEntriesAndDiffKeyByAggregateIdentityAcrossTranscriptRotation() {
        let previous = AttentionMonitor.entries(
            from: [conv("row-1", aggregateId: "pc-1", mode: "working", title: "fix login")])
        let events = AttentionMonitor.diff(
            previous: previous,
            current: [conv("row-2", aggregateId: "pc-1", mode: "needs_action", title: "fix login")])

        XCTAssertEqual(previous, ["pc-1": AttentionMonitor.Entry(mode: "working", title: "fix login")])
        XCTAssertEqual(
            events,
            [.needsAction(aggregateId: "pc-1", transcriptRowId: "row-2", title: "fix login")])
    }

    @MainActor
    func testArchivedHistoryRowsDoNotParticipateInAttentionDiff() {
        let previous = AttentionMonitor.entries(
            from: [conv("row-1", aggregateId: "pc-1", mode: "working", title: "fix login")])
        let events = AttentionMonitor.diff(
            previous: previous,
            current: [conv("row-2", aggregateId: "pc-1", mode: "needs_action", title: "fix login", archived: true)])

        XCTAssertTrue(events.isEmpty)
    }

    @MainActor
    func testResetClearsTheInMemoryAndPersistedSnapshot() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-attention-tests-\(UUID().uuidString)")
        let monitor = AttentionMonitor()
        monitor.seed(with: [conv("c1", mode: "working")])

        monitor.reset()

        XCTAssertTrue(monitor.snapshot.isEmpty)
        XCTAssertTrue(AttentionMonitor().snapshot.isEmpty)
    }

    @MainActor
    func testSupersededBackgroundEvidenceDoesNotReplaceSnapshot() async {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-attention-tests-\(UUID().uuidString)")
        let monitor = AttentionMonitor()
        monitor.seed(with: [conv("c1", mode: "working")])

        let completed = await monitor.checkAndNotify(
            [conv("c1", mode: "idle")], isCurrent: { false })

        XCTAssertFalse(completed)
        XCTAssertEqual(monitor.snapshot, ["c1": entry("working")])
    }
}
