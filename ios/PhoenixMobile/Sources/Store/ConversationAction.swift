import Foundation

/// Every user-initiated conversation operation, with its delivery policy
/// declared by the type — the app's offline thesis made structural.
///
/// The policy axis:
/// - `.outboxed` — persisted locally *before* any network I/O, carries an
///   idempotency key the server deduplicates on, auto-retried by drain
///   triggers. Sending offline is the normal case, not an error. Today
///   only chat messages qualify (they flow through Outbox, not through
///   `perform`); an action can only move here if the server endpoint is
///   idempotent-keyed like `/chat`'s `message_id`.
/// - `.onlineOnly` — the server must answer now, because the action reads
///   or transitions live server state (cancellation races the turn,
///   dismissal validates the error kind, archive frees resources). Offline
///   UX: the control is disabled or fails with an explanatory toast —
///   never silently queued, which would fabricate a stale intent to replay
///   against a state that has moved on.
///
/// To add an action: add the case, declare its policy (the switch is
/// exhaustive — the compiler forces the decision), implement it in
/// `ConversationSession.perform` (session-scoped) or `AppModel`
/// (list-scoped, e.g. archive), and wire the control with a
/// connectivity-aware disabled state.
enum ConversationAction: Equatable {
    /// Stop the in-flight agent turn.
    case cancel
    /// Clear a user-resumable error state so the conversation accepts
    /// input again. The server 409s for non-resumable errors.
    case dismissError

    enum DeliveryPolicy {
        case onlineOnly
        case outboxed
    }

    var policy: DeliveryPolicy {
        switch self {
        case .cancel, .dismissError:
            return .onlineOnly
        }
    }
}
