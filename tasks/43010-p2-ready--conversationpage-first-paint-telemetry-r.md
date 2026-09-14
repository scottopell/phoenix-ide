# Ordinary ConversationPage first-paint telemetry residual

Close the ordinary `ConversationPage` route-to-first-paint telemetry residual from stale PR #730.

Historical context:

- ProductConversation telemetry has already merged and owns related vocabulary and shape.
- The old #730 branch is stale and must not be wholesale rebased; use it only as historical evidence when tracing the residual.

Acceptance:

- Avoid parallel telemetry representations with merged ProductConversation telemetry.
- Reconcile shared `open_id`, `first_paint`, `total`, and `visible` vocabulary so ordinary `ConversationPage` and ProductConversation reports cannot drift semantically.
- Preserve a route-owned open ID for ordinary conversation opens.
- Preserve canonical slug continuity across route resolution and redirects.
- Report connected/open timing only after first paint is gated.
- Cancel teardown-before-paint opens without emitting a misleading completion.
- Track continuous visibility for the route-owned open.
- Keep the telemetry schema bounded and content-free: no prompt/message text, no transcript payloads, and no unbounded per-message detail.

Non-goals:

- Do not replay stale #730 implementation wholesale.
- Do not introduce native iOS, lifecycle, deployment, or ProductConversation behavior changes beyond vocabulary/schema reconciliation needed to prevent drift.
