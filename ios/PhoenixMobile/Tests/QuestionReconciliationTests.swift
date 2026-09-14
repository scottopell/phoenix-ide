import Foundation
import XCTest

@testable import PhoenixMobile

@MainActor
final class QuestionReconciliationTests: XCTestCase {
    private func snapshot(id: String = "conversation-a", state: String) throws -> Conversation {
        try JSONDecoder().decode(Conversation.self, from: Data(
            "{\"id\":\"\(id)\",\"slug\":\"test\",\"state\":\(state)}".utf8))
    }

    private func reconcile(_ snapshot: Conversation) -> ConversationSession.QuestionAttemptPhase {
        .reconcile(conversationId: "conversation-a", requestId: "original", result: .success(snapshot))
    }

    func testOnlyMatchingAuthoritativePendingSnapshotEnablesRetry() throws {
        let phase = reconcile(try snapshot(state:
            "{\"type\":\"awaiting_user_response\",\"request_id\":\"original\",\"questions\":[]}"))
        XCTAssertTrue(phase.canRetry)
        XCTAssertTrue(phase.canCheckStatus)
    }

    func testFailedReconciliationRevokesRetryAndKeepsStatusCheckAvailable() throws {
        var phase = reconcile(try snapshot(state:
            "{\"type\":\"awaiting_user_response\",\"request_id\":\"original\",\"questions\":[]}"))
        XCTAssertTrue(phase.canRetry)
        phase = .reconcile(conversationId: "conversation-a", requestId: "original",
                           result: .failure(URLError(.timedOut)))
        XCTAssertFalse(phase.canRetry)
        XCTAssertTrue(phase.canCheckStatus)
    }

    func testMalformedAndWrongConversationSnapshotsCannotAuthorizeRetry() throws {
        for state in ["null", "{}", "{\"type\":\"awaiting_user_response\"}",
                      "{\"type\":\"awaiting_user_response\",\"request_id\":\" \"}",
                      "{\"type\":\"unknown_future_state\"}"] {
            let phase = reconcile(try snapshot(state: state))
            XCTAssertFalse(phase.canRetry, state)
            XCTAssertTrue(phase.canCheckStatus, state)
        }
        let otherConversation = reconcile(try snapshot(id: "conversation-b", state:
            "{\"type\":\"awaiting_user_response\",\"request_id\":\"original\",\"questions\":[]}"))
        XCTAssertFalse(otherConversation.canRetry)
        XCTAssertTrue(otherConversation.canCheckStatus)
    }

    func testUnclassifiableStreamStatePreservesOperationAndRevokesRetryReadiness() {
        let action = ConversationAction.respondToQuestions(requestId: "original", answers: ["Question?": "Answer"])
        for state in [ConversationState.unknown, .questionIdentityUnavailable, .other(type: "future_state")] {
            XCTAssertTrue(ConversationSession.actionStillAwaitsOriginalState(
                action: action, origin: nil, current: state))
            let phase = ConversationSession.QuestionAttemptPhase.checkedPending.observingStreamState(state)
            XCTAssertFalse(phase.canRetry)
            XCTAssertTrue(phase.canCheckStatus)
            for inFlight in [ConversationSession.QuestionAttemptPhase.sending, .checking] {
                XCTAssertEqual(inFlight.observingStreamState(state), inFlight)
                XCTAssertFalse(inFlight.observingStreamState(state).canCheckStatus)
            }
        }
    }

    func testDifferentRequestOrResolvedStateRemovesOldOperationControls() throws {
        for state in ["{\"type\":\"idle\"}",
                      "{\"type\":\"awaiting_user_response\",\"request_id\":\"next\",\"questions\":[]}"] {
            let phase = reconcile(try snapshot(state: state))
            XCTAssertEqual(phase, .resolvedWaitingForStream)
            XCTAssertFalse(phase.canRetry)
            XCTAssertFalse(phase.canCheckStatus)
        }
        for phase in [ConversationSession.QuestionAttemptPhase.sending, .checking] {
            XCTAssertFalse(phase.canRetry)
            XCTAssertFalse(phase.canCheckStatus)
        }
    }
}
