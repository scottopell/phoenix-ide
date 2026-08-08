import Foundation

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
    case provideTaskFeedback(annotations: String)

    enum DeliveryPolicy {
        case onlineOnly
        case outboxed
    }

    var policy: DeliveryPolicy {
        switch self {
        case .cancel, .dismissError:
            return .onlineOnly
        case .approveTask, .rejectTask, .provideTaskFeedback:
            return .onlineOnly
        }
    }
}

enum TaskApprovalHandoff: String, Equatable {
    case continueInCurrentConversation = "continue_in_current_conversation"
    case startFreshWorkConversation = "start_fresh_work_conversation"
}
