import XCTest

@testable import PhoenixMobile

// Contract tests for the stopgap nudge tier's diff (REQ-IOS-018): one test
// per transition rule. The diff is pure; BGTask plumbing and notification
// delivery stay untested per the project pattern.
final class AttentionDiffTests: XCTestCase {

    private func conv(_ id: String, mode: String?, title: String = "t") -> Conversation {
        Conversation(
            id: id, slug: id, model: nil, cwd: nil,
            created_at: nil, updated_at: nil, message_count: nil,
            state: nil, state_updated_at: nil, branch_name: nil,
            task_title: title, archived: nil, project_name: nil,
            conv_mode_label: nil, presentation_mode: mode, requires_action: nil)
    }

    private func entry(_ mode: String) -> AttentionMonitor.Entry {
        AttentionMonitor.Entry(mode: mode, title: "t")
    }

    @MainActor
    func testEnteringNeedsActionNotifies() {
        let events = AttentionMonitor.diff(
            previous: ["c1": entry("working")],
            current: [conv("c1", mode: "needs_action")])
        XCTAssertEqual(events, [.needsAction(conversationId: "c1", title: "t")])
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
        XCTAssertEqual(events, [.errored(conversationId: "c1", title: "t")])
    }

    @MainActor
    func testWorkingToIdleOrDoneNotifiesFinished() {
        XCTAssertEqual(
            AttentionMonitor.diff(
                previous: ["c1": entry("working")],
                current: [conv("c1", mode: "idle")]),
            [.finished(conversationId: "c1", title: "t")])
        XCTAssertEqual(
            AttentionMonitor.diff(
                previous: ["c1": entry("working")],
                current: [conv("c1", mode: "done")]),
            [.finished(conversationId: "c1", title: "t")])
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
        XCTAssertTrue(events.contains(.needsAction(conversationId: "c1", title: "t")))
        XCTAssertTrue(events.contains(.finished(conversationId: "c2", title: "t")))
    }

    @MainActor
    func testEntriesSnapshotUsesPresentationModeAndTitle() {
        let entries = AttentionMonitor.entries(
            from: [conv("c1", mode: "working", title: "fix login")])
        XCTAssertEqual(
            entries, ["c1": AttentionMonitor.Entry(mode: "working", title: "fix login")])
    }
}
