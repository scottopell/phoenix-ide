import XCTest

@testable import PhoenixMobile

// Contract tests for the typed conversation-state decode. The contract is
// the ConvState discriminated union as serialized on the wire (mirrored by
// ConversationState in ui/src/api.ts): one test per parsing rule, plus the
// fallback rules that keep a newer server from breaking rendering.
final class ConversationStateTests: XCTestCase {

    private func parse(_ raw: String) -> ConversationState {
        let json = try? JSONDecoder().decode(JSONValue.self, from: Data(raw.utf8))
        return ConversationState.parse(json)
    }

    func testBareStringState() {
        XCTAssertEqual(parse("\"idle\""), .idle)
    }

    func testTaggedObjectState() {
        XCTAssertEqual(parse("{\"type\":\"idle\"}"), .idle)
    }

    func testLlmRequestingCarriesAttempt() {
        XCTAssertEqual(
            parse("{\"type\":\"llm_requesting\",\"attempt\":3}"),
            .llmRequesting(attempt: 3))
    }

    func testSeededLlmRequestingCollapsesToLlmRequesting() {
        XCTAssertEqual(
            parse("{\"type\":\"seeded_llm_requesting\",\"seed_message_id\":\"m1\",\"attempt\":1}"),
            .llmRequesting(attempt: 1))
    }

    func testToolExecutingCarriesToolAndCounts() {
        let raw = """
        {"type":"tool_executing",
         "current_tool":{"id":"t3","name":"bash","input":{"cmd":"ls"}},
         "remaining_tools":[{"id":"t4","name":"patch"},{"id":"t5","name":"think"}],
         "completed_results":[{"tool_use_id":"t1"},{"tool_use_id":"t2"}]}
        """
        XCTAssertEqual(
            parse(raw),
            .toolExecuting(toolName: "bash", remainingCount: 2, completedCount: 2))
    }

    func testAwaitingSubAgentsCarriesCounts() {
        let raw = """
        {"type":"awaiting_sub_agents",
         "pending":[{"id":"a"},{"id":"b"},{"id":"c"}],
         "completed_results":[{"id":"d"}]}
        """
        XCTAssertEqual(parse(raw), .awaitingSubAgents(pendingCount: 3, completedCount: 1))
    }

    func testAwaitingUserResponseCarriesTypedQuestions() {
        let raw = """
        {"type":"awaiting_user_response",
         "questions":[{"question":"Which db?","header":"DB",
                       "options":[{"label":"sqlite","description":"file-backed"},
                                  {"label":"postgres","description":""}],
                       "multiSelect":false},
                      {"question":"Which features?","header":"Feat","options":[],"multiSelect":true}]}
        """
        XCTAssertEqual(
            parse(raw),
            .awaitingUserResponse(questions: [
                UserQuestion(
                    question: "Which db?", header: "DB",
                    options: [
                        .init(label: "sqlite", description: "file-backed"),
                        .init(label: "postgres", description: ""),
                    ],
                    multiSelect: false),
                UserQuestion(
                    question: "Which features?", header: "Feat",
                    options: [], multiSelect: true),
            ]))
    }

    func testAwaitingTaskApprovalCarriesTitlePriorityPlan() {
        XCTAssertEqual(
            parse("{\"type\":\"awaiting_task_approval\",\"title\":\"Fix login\",\"priority\":\"p1\",\"plan\":\"1. do it\"}"),
            .awaitingTaskApproval(title: "Fix login", priority: "p1", plan: "1. do it"))
    }

    func testIncompleteTaskApprovalIsNonActionable() {
        XCTAssertEqual(
            parse("{\"type\":\"awaiting_task_approval\",\"title\":\"Hidden plan\"}"),
            .other(type: "awaiting_task_approval"))
    }

    func testCancellableParentStatesRemainTyped() {
        XCTAssertEqual(
            parse("{\"type\":\"awaiting_recovery\",\"message\":\"Retrying\"}"),
            .awaitingRecovery(message: "Retrying"))
        XCTAssertEqual(parse("{\"type\":\"provisioning\"}"), .provisioning)
        XCTAssertTrue(ConversationState.awaitingRecovery(message: "Retrying").isCancellable)
        XCTAssertTrue(ConversationState.provisioning.isCancellable)
    }

    func testErrorCarriesMessage() {
        XCTAssertEqual(
            parse("{\"type\":\"error\",\"message\":\"rate limited\",\"error_kind\":\"rate_limit\"}"),
            .error(message: "rate limited", kind: .rateLimit))
    }

    func testCreationFailedCarriesServerError() {
        XCTAssertEqual(
            parse("{\"type\":\"creation_failed\",\"error\":\"worktree setup failed\"}"),
            .creationFailed(message: "worktree setup failed"))
    }

    func testCancellingVariantsRemainStructurallyDistinct() {
        XCTAssertEqual(parse("{\"type\":\"cancelling\"}"), .cancelling)
        XCTAssertEqual(
            parse("{\"type\":\"cancelling_tool\",\"tool_use_id\":\"t1\"}"),
            .cancellingTool)
        XCTAssertEqual(
            parse("{\"type\":\"cancelling_sub_agents\",\"pending\":[]}"),
            .cancellingSubAgents)
    }

    func testContextExhaustedCarriesHandoffSummary() {
        XCTAssertEqual(
            parse("{\"type\":\"context_exhausted\",\"summary\":\"Continue from here\"}"),
            .contextExhausted(summary: "Continue from here"))
    }

    func testActionLockUsesStableLifecycleFamilies() {
        let origin = ConversationState.toolExecuting(
            toolName: "first", remainingCount: 2, completedCount: 0)
        let progress = ConversationState.toolExecuting(
            toolName: "second", remainingCount: 1, completedCount: 1)
        XCTAssertTrue(ConversationSession.actionStillAwaitsOriginalState(
            action: .cancel, origin: origin, current: progress))
        XCTAssertFalse(ConversationSession.actionStillAwaitsOriginalState(
            action: .cancel, origin: origin, current: .cancelling))
    }

    func testHandedOffCarriesSuccessorConversationId() {
        XCTAssertEqual(
            parse("{\"type\":\"handed_off\",\"successor_conv_id\":\"next-123\"}"),
            .handedOff(successorConversationId: "next-123"))
        XCTAssertEqual(
            parse("{\"type\":\"handed_off\"}"),
            .handedOff(successorConversationId: nil))
    }

    func testAwaitingContinuationIsWorkingButNotInteractive() {
        let state = parse("{\"type\":\"awaiting_continuation\",\"attempt\":1}")
        XCTAssertEqual(state, .awaitingContinuation)
        XCTAssertTrue(state.isKnownWorkingState)
        XCTAssertFalse(state.isCancellable)
        XCTAssertFalse(state.acceptsChatMessage)
    }

    func testChatEligibilityDistinguishesCancellationFamilies() {
        XCTAssertFalse(ConversationState.cancelling.acceptsChatMessage)
        XCTAssertTrue(ConversationState.cancellingTool.acceptsChatMessage)
        XCTAssertTrue(ConversationState.cancellingSubAgents.acceptsChatMessage)
    }

    func testCancellationAvailabilityTracksServerTransitionStates() {
        XCTAssertTrue(ConversationState.llmRequesting(attempt: 1).isCancellable)
        XCTAssertTrue(
            ConversationState.toolExecuting(
                toolName: "bash", remainingCount: 0, completedCount: 0).isCancellable)
        XCTAssertTrue(
            ConversationState.awaitingSubAgents(pendingCount: 1, completedCount: 0)
                .isCancellable)
        XCTAssertFalse(ConversationState.awaitingLlm.isCancellable)
        XCTAssertFalse(ConversationState.awaitingContinuation.isCancellable)
        XCTAssertFalse(ConversationState.cancellingTool.isCancellable)
    }

    // MARK: - Fallback rules (a newer server must degrade, not break)

    func testUnhandledVariantBecomesOtherWithTypeName() {
        XCTAssertEqual(
            parse("{\"type\":\"recoverable_continuation_failure\",\"message\":\"x\"}"),
            .other(type: "recoverable_continuation_failure"))
    }

    func testFutureUnknownVariantBecomesOther() {
        XCTAssertEqual(
            parse("{\"type\":\"quantum_reticulating\",\"spline_count\":7}"),
            .other(type: "quantum_reticulating"))
    }

    func testFallbackErrorDetailsCoverCreationAndContinuationFailures() throws {
        let creation = try JSONDecoder().decode(
            JSONValue.self,
            from: Data(#"{"type":"creation_failed","error":"directory vanished"}"#.utf8))
        let continuation = try JSONDecoder().decode(
            JSONValue.self,
            from: Data(
                #"{"type":"recoverable_continuation_failure","failure":{"message":"handoff failed"}}"#.utf8))

        XCTAssertEqual(
            StateDetailView.fallbackErrorMessage(type: "creation_failed", state: creation),
            "directory vanished")
        XCTAssertEqual(
            StateDetailView.fallbackErrorMessage(
                type: "recoverable_continuation_failure", state: continuation),
            "handoff failed")
    }

    func testMissingTypeBecomesUnknown() {
        XCTAssertEqual(parse("{\"attempt\":1}"), .unknown)
        XCTAssertEqual(ConversationState.parse(nil), .unknown)
    }

    func testFieldlessPayloadsUseDefaults() {
        // Absent detail fields degrade to defaults rather than failing.
        XCTAssertEqual(
            parse("{\"type\":\"llm_requesting\"}"), .llmRequesting(attempt: 1))
        XCTAssertEqual(
            parse("{\"type\":\"tool_executing\"}"),
            .toolExecuting(toolName: "tool", remainingCount: 0, completedCount: 0))
        XCTAssertEqual(
            parse("{\"type\":\"error\"}"),
            .error(message: "Unknown error", kind: .unknown))
    }
    func testDeliveryPolicyCoversChatArchiveAndSessionActions() {
        XCTAssertEqual(ClientOperation.chat.policy, .outboxed)
        XCTAssertEqual(ClientOperation.archive.policy, .onlineOnly)
        XCTAssertEqual(
            ClientOperation.conversationAction(.cancel).policy, .onlineOnly)
    }

    func testTaskResolutionWaitsForAuthoritativeStateChange() {
        XCTAssertTrue(
            ConversationAction.approveTask(
                handoff: .continueInCurrentConversation)
                .waitsForAuthoritativeStateChange)
        XCTAssertTrue(ConversationAction.rejectTask.waitsForAuthoritativeStateChange)
        let feedback = TaskFeedback("revise")!
        XCTAssertTrue(
            ConversationAction.provideTaskFeedback(feedback)
                .waitsForAuthoritativeStateChange)
        XCTAssertTrue(ConversationAction.cancel.waitsForAuthoritativeStateChange)
        XCTAssertTrue(ConversationAction.dismissError.waitsForAuthoritativeStateChange)
    }

    func testTaskFeedbackIsNonEmptyByConstruction() {
        XCTAssertNil(TaskFeedback(" \n "))
        XCTAssertEqual(TaskFeedback("  revise this  ")?.text, "revise this")
    }

    func testQuestionActionUnlocksWhenPromptIdentityChanges() {
        let original = ConversationState.awaitingUserResponse(questions: [
            UserQuestion(question: "First?", header: "One", options: [], multiSelect: false),
        ])
        let followUp = ConversationState.awaitingUserResponse(questions: [
            UserQuestion(question: "Next?", header: "Two", options: [], multiSelect: false),
        ])
        let action = ConversationAction.respondToQuestions(answers: ["First?": "yes"])

        XCTAssertTrue(ConversationSession.actionStillAwaitsOriginalState(
            action: action, origin: original, current: original))
        XCTAssertFalse(ConversationSession.actionStillAwaitsOriginalState(
            action: action, origin: original, current: followUp))
    }

    func testChatEligibilityMatchesInteractiveStateFamilies() {
        XCTAssertTrue(ConversationState.idle.acceptsChatMessage)
        XCTAssertTrue(
            ConversationState.error(message: "retryable", kind: .serverError)
                .acceptsChatMessage)
        XCTAssertFalse(
            ConversationState.error(message: "terminal", kind: .invalidRequest)
                .acceptsChatMessage)
        XCTAssertFalse(
            ConversationState.error(message: "unknown", kind: .unknown)
                .acceptsChatMessage)
        XCTAssertTrue(ConversationState.llmRequesting(attempt: 1).acceptsChatMessage)
        XCTAssertFalse(
            ConversationState.awaitingUserResponse(questions: []).acceptsChatMessage)
        XCTAssertFalse(
            ConversationState.awaitingTaskApproval(title: "", priority: "", plan: "")
                .acceptsChatMessage)
        XCTAssertFalse(
            ConversationState.contextExhausted(summary: nil).acceptsChatMessage)
        XCTAssertFalse(
            ConversationState.handedOff(successorConversationId: nil).acceptsChatMessage)
        XCTAssertFalse(ConversationState.terminal.acceptsChatMessage)
        XCTAssertFalse(ConversationState.other(type: "provisioning").acceptsChatMessage)
    }
}
