import XCTest

@testable import PhoenixMobile

@MainActor
final class AppModelProductConversationTests: XCTestCase {
    private func conversation(
        id: String,
        aggregateId: String? = nil,
        slug: String? = nil,
        title: String? = nil,
        taskTitle: String? = nil,
        archived: Bool? = nil,
        mode: String? = nil,
        updatedAt: String? = nil,
        runtimeRole: String? = nil
    ) -> Conversation {
        Conversation(
            id: id,
            product_conversation_id: aggregateId,
            slug: slug,
            title: title,
            model: nil,
            cwd: nil,
            created_at: nil,
            updated_at: updatedAt,
            message_count: nil,
            state: nil,
            state_updated_at: nil,
            branch_name: nil,
            task_title: taskTitle,
            archived: archived,
            project_name: nil,
            conv_mode_label: nil,
            presentation_mode: mode,
            requires_action: nil,
            transcript_generation: nil,
            runtime_role: runtimeRole)
    }

    private func persistReadableSnapshot(conversation: Conversation) {
        struct Snapshot: Codable {
            var conversation: Conversation?
            var messages: [Message]
            var lastSequenceId: Int64
            var transcriptGeneration: Int64?
            var syncedAt: Date?
        }

        DiskStore.saveVersioned(
            Snapshot(
                conversation: conversation,
                messages: [],
                lastSequenceId: 0,
                transcriptGeneration: 1,
                syncedAt: Date()),
            name: "conv-\(conversation.id)",
            version: 1)
    }

    func testBackgroundIntegrationPreservesAuthoritativeAggregateIdentityAfterLegacyCache() {
        let model = AppModel()
        let aggregateProjection = conversation(
            id: "latest-row",
            aggregateId: "pc-1",
            slug: "canonical-root",
            title: "Canonical Title",
            updatedAt: "2025-01-02T03:04:05Z")
        let liveTranscriptUpdate = conversation(
            id: "newer-row",
            slug: "transcript-slug",
            title: "Transcript Title",
            updatedAt: "2025-01-02T05:04:05Z")

        let merged = model.integrateBackgroundConversationUpdate(
            existing: aggregateProjection,
            update: liveTranscriptUpdate)

        XCTAssertEqual(merged.product_conversation_id, "pc-1")
        XCTAssertEqual(merged.aggregateIdentity, "pc-1")
        XCTAssertEqual(merged.id, "latest-row")
    }

    func testBackgroundIntegrationPreservesCanonicalRootMetadataAcrossLiveUpdate() {
        let model = AppModel()
        let aggregateProjection = conversation(
            id: "latest-row",
            aggregateId: "pc-1",
            slug: "canonical-root",
            title: "Canonical Title",
            taskTitle: "Canonical Task",
            archived: false,
            mode: "working",
            updatedAt: "2025-01-02T03:04:05Z")
        let liveTranscriptUpdate = conversation(
            id: "newer-row",
            slug: "ephemeral-transcript-slug",
            title: "Ephemeral Transcript Title",
            taskTitle: nil,
            archived: true,
            mode: "needs_action",
            updatedAt: "2025-01-02T06:04:05Z")

        let merged = model.integrateBackgroundConversationUpdate(
            existing: aggregateProjection,
            update: liveTranscriptUpdate)

        XCTAssertEqual(merged.product_conversation_id, "pc-1")
        XCTAssertEqual(merged.slug, "canonical-root")
        XCTAssertEqual(merged.title, "Canonical Title")
        XCTAssertEqual(merged.task_title, "Canonical Task")
        XCTAssertEqual(merged.archived, false)
        XCTAssertEqual(merged.presentation_mode, "needs_action")
        XCTAssertEqual(merged.id, "latest-row")
    }

    func testBackgroundIntegrationIgnoresDivergentSuccessorTaskTitle() {
        let model = AppModel()
        let aggregateProjection = conversation(
            id: "latest-row",
            aggregateId: "pc-1",
            slug: "canonical-root",
            title: "Canonical Title",
            taskTitle: "Canonical Task")
        let liveTranscriptUpdate = conversation(
            id: "successor-row",
            slug: "successor-slug",
            title: "Successor Title",
            taskTitle: "Successor Task Title")

        let merged = model.integrateBackgroundConversationUpdate(
            existing: aggregateProjection,
            update: liveTranscriptUpdate)

        XCTAssertEqual(merged.title, "Canonical Title")
        XCTAssertEqual(merged.task_title, "Canonical Task")
    }


    func testColdRefreshDoesNotReinjectCoordinatorSnapshotIntoAuthoritativeList() async {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let coordinator = conversation(
            id: "coordinator-row",
            slug: "fleet",
            title: "Fleet",
            runtimeRole: "coordinator")
        persistReadableSnapshot(conversation: coordinator)
        UserDefaults.standard.set("coordinator-row", forKey: "phoenix.coordinatorConversationId")

        let model = AppModel()

        XCTAssertEqual(model.listStore.conversations.filter(\.isCoordinator).count, 0)
        let coordinatorId = await model.openCoordinator()
        XCTAssertEqual(coordinatorId, "coordinator-row")
    }

    func testOfflineNotificationNavigationUsesCachedAggregateMember() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let predecessor = conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        persistReadableSnapshot(conversation: predecessor)
        let model = AppModel()
        model.listStore.upsert(predecessor)
        model.listStore.upsert(conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        model.connectivity.setOnlineForTesting(false)

        let resolved = model.resolvedNavigationConversationId(
            aggregateId: model.listStore.aggregateId(forTranscriptRowId: "row-2"),
            latestTranscriptRowId: "row-2")

        XCTAssertEqual(resolved, "row-1")
    }
    func testOfflineHandoffNavigationUsesCachedAggregateMember() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let predecessor = conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        persistReadableSnapshot(conversation: predecessor)
        let model = AppModel()
        model.listStore.upsert(predecessor)
        model.listStore.upsert(conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        model.connectivity.setOnlineForTesting(false)

        let resolved = model.resolvedNavigationConversationId(
            aggregateId: model.listStore.aggregateId(forTranscriptRowId: "row-2"),
            latestTranscriptRowId: "row-2")

        XCTAssertEqual(resolved, "row-1")
    }

    func testOfflineNavigationUsesCachedAggregateMemberWhenLatestSnapshotMissing() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let predecessor = conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        persistReadableSnapshot(conversation: predecessor)
        let model = AppModel()
        model.listStore.upsert(predecessor)
        model.listStore.upsert(conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root"))
        let aggregateConversation = model.listStore.conversations.first!
        model.connectivity.setOnlineForTesting(false)

        XCTAssertEqual(model.navigationConversationId(for: aggregateConversation), "row-1")
    }

    func testOfflineNavigationUsesCachedAggregateMemberAfterRestart() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-appmodel-tests-\(UUID().uuidString)")
        let predecessor = conversation(id: "row-1", aggregateId: "pc-1", slug: "root", title: "Root")
        persistReadableSnapshot(conversation: predecessor)
        let first = AppModel()
        first.listStore.upsert(predecessor)
        first.listStore.applyExternal(
            [conversation(id: "row-2", aggregateId: "pc-1", slug: "root", title: "Root")],
            startedAt: first.listStore.externalRefreshToken())

        let reloaded = AppModel()
        reloaded.connectivity.setOnlineForTesting(false)
        let aggregateConversation = reloaded.listStore.conversations.first!

        XCTAssertEqual(reloaded.navigationConversationId(for: aggregateConversation), "row-1")
    }
}
