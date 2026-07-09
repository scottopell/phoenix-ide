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
/// To promote a variant from `.other` to a typed case:
///   1. Add the case with the fields the UI needs (check the union in
///      ui/src/api.ts and `ConvState` in phoenix-core sm_state.rs).
///   2. Parse it in `parse(_:)`.
///   3. Render it in `StateDetailView` (Views/StateViews.swift).
///   4. Add a decoding test in ConversationStateTests, one per shape rule.
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
    case awaitingUserResponse(questionCount: Int, firstQuestion: String?)
    case awaitingTaskApproval(title: String, priority: String, plan: String)
    case error(message: String)
    case contextExhausted
    /// Covers `cancelling`, `cancelling_tool`, `cancelling_sub_agents`.
    case cancelling
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
        case "awaiting_user_response":
            let questions = json["questions"]?.arrayValue ?? []
            return .awaitingUserResponse(
                questionCount: questions.count,
                firstQuestion: questions.first?["question"]?.stringValue)
        case "awaiting_task_approval":
            return .awaitingTaskApproval(
                title: json["title"]?.stringValue ?? "",
                priority: json["priority"]?.stringValue ?? "",
                plan: json["plan"]?.stringValue ?? "")
        case "error":
            return .error(message: json["message"]?.stringValue ?? "Unknown error")
        case "context_exhausted":
            return .contextExhausted
        case "cancelling", "cancelling_tool", "cancelling_sub_agents":
            return .cancelling
        case "terminal":
            return .terminal
        case "handed_off":
            return .handedOff
        default:
            return .other(type: type)
        }
    }
}
