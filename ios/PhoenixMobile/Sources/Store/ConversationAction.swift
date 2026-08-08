import Foundation

struct TaskFeedback: Equatable {
    let text: String

    init?(_ raw: String) {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        self.text = text
    }
}

/// Session-scoped operations invoked by conversation controls.
enum ConversationAction: Equatable {
    /// Stop the in-flight agent turn.
    case cancel
    /// Clear a user-resumable error state so the conversation accepts
    /// input again. The server 409s for non-resumable errors.
    case dismissError
    /// Approve the proposed task plan (awaiting_task_approval only; the
    /// server 400s otherwise, e.g. when another client already decided).
    case approveTask(handoff: TaskApprovalHandoff)
    /// Reject the proposed task plan.
    case rejectTask
    /// Send the plan back with free-text change requests; the agent
    /// revises and re-proposes.
    case provideTaskFeedback(TaskFeedback)
    /// Answer the agent's questions (awaiting_user_response). Answers are
    /// keyed by question text, encoded per QuestionAnswers.
    case respondToQuestions(answers: [String: String])
    /// Dismiss the questions without answering and return the conversation to idle.
    case dismissQuestion

    var waitsForAuthoritativeStateChange: Bool {
        true
    }
}

/// Delivery classification for user-authored iOS operations.
enum ClientOperation {
    case chat
    case archive
    case conversationAction(ConversationAction)

    enum DeliveryPolicy: Equatable {
        case onlineOnly
        case outboxed
    }

    var policy: DeliveryPolicy {
        switch self {
        case .chat:
            return .outboxed
        case .archive, .conversationAction:
            return .onlineOnly
        }
    }
}

enum TaskApprovalHandoff: String, Equatable {
    case continueInCurrentConversation = "continue_in_current_conversation"
    case startFreshWorkConversation = "start_fresh_work_conversation"
}
