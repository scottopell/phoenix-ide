import Foundation

// Wire types for the Phoenix HTTP API. Field names match the server's JSON
// (snake_case) so no key-mapping strategy is needed. Fields the app doesn't
// consume are omitted — unknown keys are ignored by JSONDecoder.

struct Conversation: Codable, Identifiable, Equatable, Hashable {
    var id: String
    var slug: String
    var model: String?
    var cwd: String?
    var created_at: String?
    var updated_at: String?
    var message_count: Int?
    var state: JSONValue?
    var state_updated_at: String?
    var branch_name: String?
    var task_title: String?
    var archived: Bool?
    var project_name: String?
    var conv_mode_label: String?
    /// Server-derived display mode: idle | working | needs_action | error |
    /// done. Authoritative over any client-side guess from `state` — e.g. a
    /// `context_exhausted` conversation is needs-action until continued,
    /// done after, which is not decidable from the state type alone.
    var presentation_mode: String?
    /// Server-derived "user must act" flag paired with presentation_mode.
    var requires_action: Bool?

    /// `state` is a discriminated union on the wire — either a bare string
    /// or `{ "type": "...", ... }`. Both shapes collapse to the type name.
    var stateType: String? {
        if let s = state?.stringValue { return s }
        return state?["type"]?.stringValue
    }

    /// Short label for list rows: task title if set, else the tail of cwd.
    var displayTitle: String {
        if let title = task_title, !title.isEmpty { return title }
        if let cwd, !cwd.isEmpty {
            return (cwd as NSString).lastPathComponent
        }
        return slug
    }

    var updatedAtDate: Date? {
        updated_at.flatMap { Self.parseDate($0) }
    }

    static func parseDate(_ s: String) -> Date? {
        // Server timestamps are RFC3339, sometimes with fractional seconds.
        let withFraction = ISO8601DateFormatter()
        withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = withFraction.date(from: s) { return d }
        let plain = ISO8601DateFormatter()
        return plain.date(from: s)
    }
}

struct Message: Codable, Identifiable, Equatable {
    var message_id: String
    var conversation_id: String?
    var sequence_id: Int64
    var message_type: String
    var content: JSONValue
    var display_data: JSONValue?
    var created_at: String?

    var id: String { message_id }

    var createdAtDate: Date? {
        created_at.flatMap { Conversation.parseDate($0) }
    }
}

struct ImagePayload: Codable, Equatable {
    var data: String
    var media_type: String
}

// MARK: - Response envelopes

struct AuthStatusResponse: Codable {
    var auth_required: Bool
    var authenticated: Bool
}

struct ConversationListResponse: Codable {
    var conversations: [Conversation]
}

struct ConversationResponse: Codable {
    var conversation: Conversation
}

struct ConversationWithMessagesResponse: Codable {
    var conversation: Conversation
    var messages: [Message]
    var agent_working: Bool?
    var presentation_mode: String?
}

struct ChatResponse: Codable {
    var queued: Bool
    /// Present and true when the conversation was busy and the message was
    /// accepted onto the steering queue instead of processed immediately.
    var steering: Bool?
}

struct CancelResponse: Codable {
    var ok: Bool
    var no_op: Bool?
}

struct ValidateCwdResponse: Codable {
    var valid: Bool
    var error: String?
    var is_git: Bool?
}

struct ModelsResponse: Codable {
    var models: [JSONValue]
    var `default`: String?

    /// Model IDs, tolerant of the entry shape (string or object with `id`).
    var modelIDs: [String] {
        models.compactMap { $0.stringValue ?? $0["id"]?.stringValue }
    }
}
