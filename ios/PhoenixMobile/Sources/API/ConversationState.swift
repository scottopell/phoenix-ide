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
    case awaitingUserResponse(questions: [UserQuestion])
    case awaitingTaskApproval(title: String, priority: String, plan: String)
    case error(message: String)
    case creationFailed(message: String)
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
            let questions = (json["questions"]?.arrayValue ?? [])
                .compactMap(UserQuestion.parse)
            return .awaitingUserResponse(questions: questions)
        case "awaiting_task_approval":
            return .awaitingTaskApproval(
                title: json["title"]?.stringValue ?? "",
                priority: json["priority"]?.stringValue ?? "",
                plan: json["plan"]?.stringValue ?? "")
        case "error":
            return .error(message: json["message"]?.stringValue ?? "Unknown error")
        case "creation_failed":
            return .creationFailed(
                message: json["error"]?.stringValue
                    ?? json["message"]?.stringValue
                    ?? "Conversation creation failed")
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

    /// Mirrors the server's chat-acceptance surface at the client boundary.
    /// Working states remain sendable because the server accepts them as
    /// steering; blocking, provisioning, and terminal states do not.
    var acceptsChatMessage: Bool {
        switch self {
        case .idle, .error, .llmRequesting, .toolExecuting,
             .awaitingSubAgents, .cancelling:
            return true
        case .awaitingLlm, .awaitingUserResponse, .awaitingTaskApproval,
             .creationFailed, .contextExhausted, .terminal, .handedOff,
             .other, .unknown:
            return false
        }
    }

    var isKnownWorkingState: Bool {
        switch self {
        case .awaitingLlm, .llmRequesting, .toolExecuting,
             .awaitingSubAgents, .cancelling:
            return true
        default:
            return false
        }
    }
}

/// One question from an awaiting_user_response state (mirror of
/// `UserQuestion` in ui/src/api.ts). Answer encoding rules live in
/// QuestionAnswers.
struct UserQuestion: Equatable {
    struct Option: Equatable {
        var label: String
        var description: String
    }

    var question: String
    var header: String
    var options: [Option]
    var multiSelect: Bool

    static func parse(_ json: JSONValue) -> UserQuestion? {
        guard let question = json["question"]?.stringValue else { return nil }
        return UserQuestion(
            question: question,
            header: json["header"]?.stringValue ?? "",
            options: (json["options"]?.arrayValue ?? []).compactMap { option in
                guard let label = option["label"]?.stringValue else { return nil }
                return Option(
                    label: label,
                    description: option["description"]?.stringValue ?? "")
            },
            multiSelect: json["multiSelect"]?.boolValue ?? false)
    }
}

/// Pure answer-map encoding, matching the web QuestionPanel's contract:
/// answers are keyed by question text; a single-select answer is the chosen
/// option label (or the free "Other" text); a multi-select answer joins the
/// chosen labels with ", ", appending trimmed "Other" text when present.
enum QuestionAnswers {
    /// nil when any question is unanswered (no selection and no Other text).
    static func encode(
        questions: [UserQuestion],
        selections: [String: Set<String>],
        otherTexts: [String: String]
    ) -> [String: String]? {
        var result: [String: String] = [:]
        for question in questions {
            let selected = selections[question.question] ?? []
            let other = (otherTexts[question.question] ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if question.multiSelect {
                var labels = question.options.map(\.label).filter(selected.contains)
                if !other.isEmpty { labels.append(other) }
                guard !labels.isEmpty else { return nil }
                result[question.question] = labels.joined(separator: ", ")
            } else {
                if let label = question.options.map(\.label).first(where: selected.contains) {
                    result[question.question] = label
                } else if !other.isEmpty {
                    result[question.question] = other
                } else {
                    return nil
                }
            }
        }
        return result
    }
}
