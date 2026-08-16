# Reconnect and resnapshot ambiguous direct turns

Replace same-stream recovery for ambiguous direct-turn materialization or post-materialization recovery with a bounded incarnation-abandonment policy. Preserve durable repository authority and exact claim semantics, drop all old broadcaster sender ownership before any replacement runtime is created, and let reconnect deliver a fresh SQLite-authoritative Init.

## Acceptance criteria

- Ambiguous abandonment closes the old SSE subscriber without broadcaster or reserved-cursor transfer and emits no filler or duplicate event.
- Reconnect creates a fresh stream incarnation whose Init reflects committed versus uncommitted SQLite truth; a committed canonical user message appears exactly once.
- Claim release requires an exact typed repository result proving non-commit for the exact accepted turn and generation; ambiguous or committed materialization remains retained/owed.
- Provider dispatch is at most once, confirmed non-commit remains retryable, and one failed conversation does not block another conversation or worker readiness.
- Ordinary replay behavior remains intact for coherent, non-abandoned reconnects.
- Requirements, Allium, executive status, and a new ADR define the policy; no schema, persisted SSE events, retry service, bootstrap watch, or same-stream repair machinery is added.
