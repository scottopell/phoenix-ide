import Foundation

// Wire types for the Phoenix HTTP API. Field names match the server's JSON
// (snake_case) so no key-mapping strategy is needed. Fields the app doesn't
// consume are omitted — unknown keys are ignored by JSONDecoder.

struct Conversation: Codable, Identifiable, Equatable, Hashable, Sendable {
    /// Transcript-row identity. This remains the owner for `ConversationSession`,
    /// SSE routes, message snapshots, and outboxes.
    var id: String
    /// Durable aggregate identity for user-facing list/cache ownership.
    /// additive-optional: legacy `/api/conversations` rows omit it; nil means
    /// the transcript-row id is the only available identity.
    var product_conversation_id: String?
    var slug: String?
    var title: String?
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
    var transcript_generation: Int64?
    // additive-optional: older persisted rows omit role; nil is correct.
    var runtime_role: String?

    /// ProductConversation aggregate identity when present; transcript-row
    /// fallback preserves compatibility with cached legacy rows.
    var aggregateIdentity: String { product_conversation_id ?? id }

    /// Canonical transcript row for list navigation. Aggregate-backed rows use
    /// the latest transcript row; legacy rows fall back to their own id.
    var transcriptRowIdentity: String { id }

    var isCoordinator: Bool { runtime_role == "coordinator" }

    /// `state` is a discriminated union on the wire — either a bare string
    /// or `{ "type": "...", ... }`. Both shapes collapse to the type name.
    var stateType: String? {
        if let s = state?.stringValue { return s }
        return state?["type"]?.stringValue
    }

    /// Short label for list rows: task title, server title, then cwd tail.
    var displayTitle: String {
        if let title = task_title, !title.isEmpty { return title }
        if let title, !title.isEmpty { return title }
        if let cwd, !cwd.isEmpty {
            return (cwd as NSString).lastPathComponent
        }
        if let slug, !slug.isEmpty { return slug }
        return id
    }

    /// Stable secondary label for list rows. Legacy/imported rows can have
    /// a null slug, so identity is the lossless floor.
    var displaySlug: String { slug.flatMap { $0.isEmpty ? nil : $0 } ?? id }

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

struct ProductConversationListResponse: Codable, Equatable, Sendable {
    var product_conversations: [ProductConversationListRow]
}

struct ProductConversationListRow: Codable, Equatable, Sendable {
    var product_conversation_id: String
    var canonical_route: String
    var canonical_root: ProductConversationTranscriptRow
    var ordinary_lifecycle: ProductConversationOrdinaryLifecycle
    var latest_transcript_row_id: String
    var updated_at: String
    var presentation: ProductConversationPresentation
}

struct ProductConversationSnapshot: Codable, Equatable, Sendable {
    var product_conversation_id: String
    var close: ProductConversationClose?
    var canonical_route: String
    var requested_transcript_row_id: String
    var canonical_root: ProductConversationTranscriptRow
    var ordinary_lifecycle: ProductConversationOrdinaryLifecycle
    var latest_transcript_row_id: String
    var writable_transcript_row_id: String?
    var updated_at: String
    var presentation: ProductConversationPresentation
    var work_identity: ProductConversationWorkIdentity?
    var source: ProductConversationSource?
    var chain_qa_compatibility: ProductConversationChainQaCompatibility?
    var segments: [ProductConversationSegment]
    var before: String?
    var has_older: Bool
}

struct ProductConversationTranscriptRow: Codable, Equatable, Sendable {
    var transcript_row_id: String
    var slug: String?
    var title: String?
}

enum ProductConversationOrdinaryLifecycle: String, Codable, Equatable, Sendable {
    case open
    case history
}

enum ProductConversationPresentation: Codable, Equatable, Sendable {
    case needsAction(displayName: String)
    case state(displayName: String, presentationMode: String)

    private enum CodingKeys: String, CodingKey {
        case kind
        case display_name
        case presentation_mode
    }

    private enum Kind: String, Codable {
        case needs_action
        case state
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(Kind.self, forKey: .kind) {
        case .needs_action:
            self = .needsAction(displayName: try container.decode(String.self, forKey: .display_name))
        case .state:
            self = .state(
                displayName: try container.decode(String.self, forKey: .display_name),
                presentationMode: try container.decode(String.self, forKey: .presentation_mode))
        }
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .needsAction(let displayName):
            try container.encode(Kind.needs_action, forKey: .kind)
            try container.encode(displayName, forKey: .display_name)
        case .state(let displayName, let presentationMode):
            try container.encode(Kind.state, forKey: .kind)
            try container.encode(displayName, forKey: .display_name)
            try container.encode(presentationMode, forKey: .presentation_mode)
        }
    }
}

struct ProductConversationWorkIdentity: Codable, Equatable, Sendable {
    var work_transcript_row_id: String
    var worktree_path: String
    var branch_name: String?
    var base_branch: String
    var task_id: String?
    var task_title: String?
}

enum ProductConversationSource: Codable, Equatable, Sendable {
    case present(
        sourceProductConversationId: String,
        sourceConversationId: String,
        relation: ProductConversationSourceRelation,
        relationKey: String)
    case deleted(
        sourceProductConversationId: String,
        sourceConversationId: String,
        relation: ProductConversationSourceRelation,
        relationKey: String)

    private enum CodingKeys: String, CodingKey {
        case status
        case source_product_conversation_id
        case source_conversation_id
        case relation
        case relation_key
    }

    private enum Status: String, Codable {
        case present
        case deleted
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let sourceProductConversationId = try container.decode(String.self, forKey: .source_product_conversation_id)
        let sourceConversationId = try container.decode(String.self, forKey: .source_conversation_id)
        let relation = try container.decode(ProductConversationSourceRelation.self, forKey: .relation)
        let relationKey = try container.decode(String.self, forKey: .relation_key)
        switch try container.decode(Status.self, forKey: .status) {
        case .present:
            self = .present(
                sourceProductConversationId: sourceProductConversationId,
                sourceConversationId: sourceConversationId,
                relation: relation,
                relationKey: relationKey)
        case .deleted:
            self = .deleted(
                sourceProductConversationId: sourceProductConversationId,
                sourceConversationId: sourceConversationId,
                relation: relation,
                relationKey: relationKey)
        }
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .present(let sourceProductConversationId, let sourceConversationId, let relation, let relationKey):
            try container.encode(Status.present, forKey: .status)
            try container.encode(sourceProductConversationId, forKey: .source_product_conversation_id)
            try container.encode(sourceConversationId, forKey: .source_conversation_id)
            try container.encode(relation, forKey: .relation)
            try container.encode(relationKey, forKey: .relation_key)
        case .deleted(let sourceProductConversationId, let sourceConversationId, let relation, let relationKey):
            try container.encode(Status.deleted, forKey: .status)
            try container.encode(sourceProductConversationId, forKey: .source_product_conversation_id)
            try container.encode(sourceConversationId, forKey: .source_conversation_id)
            try container.encode(relation, forKey: .relation)
            try container.encode(relationKey, forKey: .relation_key)
        }
    }
}

enum ProductConversationSourceRelation: String, Codable, Equatable, Sendable {
    case approved_task
}

struct ProductConversationChainQaCompatibility: Codable, Equatable, Sendable {
    var url: String
    var root_transcript_row_id: String
}

struct ProductConversationSegment: Codable, Equatable, Sendable {
    var segment_ordinal: Int64
    var transcript_row_id: String
    var slug: String?
    var title: String?
    var messages: [Message]
    var handoff: ProductConversationHandoff?
}

enum ProductConversationHandoff: Codable, Equatable, Sendable {
    case completed(
        predecessorTranscriptRowId: String,
        successorTranscriptRowId: String,
        continuationMessageId: String,
        acceptedSuccessorMessageId: String,
        summary: String)
    case historical(
        predecessorTranscriptRowId: String,
        successorTranscriptRowId: String,
        continuationMessageId: String,
        summary: String)

    private enum CodingKeys: String, CodingKey {
        case kind
        case predecessor_transcript_row_id
        case successor_transcript_row_id
        case continuation_message_id
        case accepted_successor_message_id
        case summary
    }

    private enum Kind: String, Codable {
        case completed
        case historical
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(Kind.self, forKey: .kind) {
        case .completed:
            self = .completed(
                predecessorTranscriptRowId: try container.decode(String.self, forKey: .predecessor_transcript_row_id),
                successorTranscriptRowId: try container.decode(String.self, forKey: .successor_transcript_row_id),
                continuationMessageId: try container.decode(String.self, forKey: .continuation_message_id),
                acceptedSuccessorMessageId: try container.decode(String.self, forKey: .accepted_successor_message_id),
                summary: try container.decode(String.self, forKey: .summary))
        case .historical:
            self = .historical(
                predecessorTranscriptRowId: try container.decode(String.self, forKey: .predecessor_transcript_row_id),
                successorTranscriptRowId: try container.decode(String.self, forKey: .successor_transcript_row_id),
                continuationMessageId: try container.decode(String.self, forKey: .continuation_message_id),
                summary: try container.decode(String.self, forKey: .summary))
        }
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .completed(
            let predecessorTranscriptRowId,
            let successorTranscriptRowId,
            let continuationMessageId,
            let acceptedSuccessorMessageId,
            let summary
        ):
            try container.encode(Kind.completed, forKey: .kind)
            try container.encode(predecessorTranscriptRowId, forKey: .predecessor_transcript_row_id)
            try container.encode(successorTranscriptRowId, forKey: .successor_transcript_row_id)
            try container.encode(continuationMessageId, forKey: .continuation_message_id)
            try container.encode(acceptedSuccessorMessageId, forKey: .accepted_successor_message_id)
            try container.encode(summary, forKey: .summary)
        case .historical(
            let predecessorTranscriptRowId,
            let successorTranscriptRowId,
            let continuationMessageId,
            let summary
        ):
            try container.encode(Kind.historical, forKey: .kind)
            try container.encode(predecessorTranscriptRowId, forKey: .predecessor_transcript_row_id)
            try container.encode(successorTranscriptRowId, forKey: .successor_transcript_row_id)
            try container.encode(continuationMessageId, forKey: .continuation_message_id)
            try container.encode(summary, forKey: .summary)
        }
    }
}

struct ProductConversationClose: Codable, Equatable, Sendable {
    var attempt_id: String
    var phase: ProductConversationClosePhase
    var confirmation_snapshot: ProductConversationCloseInspection?
    var inspections: [ProductConversationCloseInspection]
    var losses: [ProductConversationCloseLoss]
    var residuals: [ProductConversationCloseResidual]
}

enum ProductConversationClosePhase: String, Codable, Equatable, Sendable {
    case awaiting_blocker_resolution
    case awaiting_stop_work_confirmation
    case settling_active_work
    case cancel_requested_during_settlement
    case awaiting_retirement_inspection
    case awaiting_loss_confirmation
    case retirement_requested
    case needs_repair
    case completed
}

struct ProductConversationCloseInspection: Codable, Equatable, Sendable {
    var scope: String
    var generation: String
    var fingerprint: String
}

struct ProductConversationCloseLoss: Codable, Equatable, Sendable {
    var scope: String
    var generation: String
    var category: String
    var identity: String
}

struct ProductConversationCloseResidual: Codable, Equatable, Sendable {
    var scope: String
    var resource_kind: String
    var identity: String
    var reason: String
    var detail: String?
}

struct Message: Codable, Identifiable, Equatable, Sendable {
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

struct ImagePayload: Codable, Equatable, Sendable {
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
    /// True when this message id was persisted by an earlier request. The
    /// duplicate POST produces no new SSE message, so the client must fetch
    /// the authoritative row explicitly before pruning its outbox entry.
    var already_persisted: Bool?
}

struct ReconcileAcceptedMessagesResponse: Codable {
    var conversation_idle: Bool
    var entries: [AcceptedMessageReconciliation]
}

struct AcceptedMessageReconciliation: Codable {
    enum Status: String, Codable {
        case persisted
        case steeringQueued = "steering_queued"
        case absent
    }

    var message_id: String
    var status: Status
    var message: Message?
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
    var llm_configured: Bool?

    /// Model IDs, tolerant of the entry shape (string or object with `id`).
    var modelIDs: [String] {
        models.compactMap { $0.stringValue ?? $0["id"]?.stringValue }
    }
}
