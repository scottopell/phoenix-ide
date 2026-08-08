import Foundation

/// Typed view of the server's conversation state — the discriminated union
/// `ConvState` (mirrored by `ConversationState` in ui/src/api.ts).
///
/// The pattern (shared with PhoenixEvent and the tool-renderer dispatch):
/// variants this client understands become typed cases with just the fields
/// the UI consumes; a recognized-but-unhandled variant lands in
/// `.other(type:)` so it can still be labeled; an unparseable payload lands
/// in `.unknown`. A newer server can therefore never break rendering — it
/// only degrades it.
///
/// Note: whether the agent is *busy* comes from the server's
/// `presentation_mode`, not from this type — don't re-derive it here.
enum ConversationState: Equatable {
    case idle
    case awaitingLlm
    /// Covers `llm_requesting` and `seeded_llm_requesting` (identical for
    /// display purposes).
    case llmRequesting(attempt: Int)
    case toolExecuting(toolName: String, remainingCount: Int, completedCount: Int)
    case awaitingSubAgents(pendingCount: Int, completedCount: Int)
    case awaitingContinuation
    case awaitingUserResponse(questionCount: Int, firstQuestion: String?)
    case awaitingTaskApproval(title: String, priority: String, plan: String)
    case awaitingCommissionReviewApproval
    case awaitingRecovery(message: String)
    case provisioning
    case error(message: String)
    case contextExhausted
    case cancelling
    case cancellingTool
    case cancellingSubAgents
    case terminal
    case handedOff
    /// Recognized envelope, variant without a typed case yet.
    case other(type: String)
    /// Payload didn't match the envelope at all (nil, or no type name).
    case unknown

    /// Parse the wire `state` value: either a bare string (legacy) or a
    /// `{ "type": ..., ...fields }` object.
    static func parse(_ json: JSONValue?) -> ConversationState {
        guard let json else { return .unknown }
        guard let type = json.stringValue ?? json["type"]?.stringValue else {
            return .unknown
        }
        switch type {
        case "idle":
            return .idle
        case "awaiting_llm":
            return .awaitingLlm
        case "llm_requesting", "seeded_llm_requesting":
            return .llmRequesting(attempt: json["attempt"]?.intValue ?? 1)
        case "tool_executing":
            return .toolExecuting(
                toolName: json["current_tool"]?["name"]?.stringValue ?? "tool",
                remainingCount: json["remaining_tools"]?.arrayValue?.count ?? 0,
                completedCount: json["completed_results"]?.arrayValue?.count ?? 0)
        case "awaiting_sub_agents":
            return .awaitingSubAgents(
                pendingCount: json["pending"]?.arrayValue?.count ?? 0,
                completedCount: json["completed_results"]?.arrayValue?.count ?? 0)
        case "awaiting_continuation":
            return .awaitingContinuation
        case "awaiting_user_response":
            let questions = json["questions"]?.arrayValue ?? []
            return .awaitingUserResponse(
                questionCount: questions.count,
                firstQuestion: questions.first?["question"]?.stringValue)
        case "awaiting_task_approval":
            guard let title = json["title"]?.stringValue,
                  let priority = json["priority"]?.stringValue,
                  let plan = json["plan"]?.stringValue
            else { return .other(type: type) }
            return .awaitingTaskApproval(
                title: title, priority: priority, plan: plan)
        case "awaiting_commission_review_approval":
            return .awaitingCommissionReviewApproval
        case "awaiting_recovery":
            return .awaitingRecovery(
                message: json["message"]?.stringValue ?? "Recovery in progress")
        case "provisioning":
            return .provisioning
        case "error":
            return .error(message: json["message"]?.stringValue ?? "Unknown error")
        case "context_exhausted":
            return .contextExhausted
        case "cancelling":
            return .cancelling
        case "cancelling_tool":
            return .cancellingTool
        case "cancelling_sub_agents":
            return .cancellingSubAgents
        case "terminal":
            return .terminal
        case "handed_off":
            return .handedOff
        default:
            return .other(type: type)
        }
    }

    /// Mirrors the server's chat/steering acceptance families. Plain
    /// cancellation rejects chat, while tool and sub-agent cancellation
    /// still accept a follow-up for after the current turn.
    var acceptsChatMessage: Bool {
        switch self {
        case .idle, .error, .llmRequesting, .toolExecuting,
             .awaitingSubAgents, .cancellingTool, .cancellingSubAgents:
            return true
        case .awaitingLlm, .awaitingContinuation, .cancelling,
             .awaitingUserResponse, .awaitingTaskApproval,
             .awaitingCommissionReviewApproval, .awaitingRecovery, .provisioning,
             .contextExhausted,
             .terminal, .handedOff, .other, .unknown:
            return false
        }
    }

    var isCancellable: Bool {
        switch self {
        case .llmRequesting, .toolExecuting, .awaitingSubAgents,
             .awaitingTaskApproval, .awaitingCommissionReviewApproval,
             .awaitingRecovery, .provisioning:
            return true
        case .idle, .awaitingLlm, .awaitingContinuation,
             .awaitingUserResponse, .error, .contextExhausted, .cancelling,
             .cancellingTool, .cancellingSubAgents, .terminal, .handedOff,
             .other, .unknown:
            return false
        }
    }

    var isKnownWorkingState: Bool {
        switch self {
        case .awaitingLlm, .llmRequesting, .toolExecuting,
             .awaitingSubAgents, .awaitingContinuation, .awaitingRecovery,
             .provisioning, .cancelling,
             .cancellingTool, .cancellingSubAgents:
            return true
        default:
            return false
        }
    }
}
