# Make SSE init the sole initial conversation authority

## User journey
Opening a conversation resolves the route, opens SSE, and renders authoritative metadata plus a bounded newest transcript tail from one stable init snapshot. Older history remains server-owned and loads only when scrolling upward or resolving a deep link.

## Scope
- Add explicit `tail | complete` transcript coverage to the typed SSE init contract.
- Treat validated init as authoritative conversation metadata, including archive/action gating.
- Replace the full metadata route bootstrap with the smallest route-resolution contract; delete the old metadata endpoints/client methods if no named consumer remains.
- Remove the initial latest-message REST request from ConversationPage.
- Remove `messages_after_floor` end to end: client request mode/query, server query/selection branch, DB stream read path, tests, and specs.
- Preserve `after_event_sequence`, ReplayRing replay, stable subscribe-before-snapshot ordering, transcript generation, pending events, and stale-route/epoch guards.
- Preserve lazy REST older-history/deep-link loading; never eagerly load the complete transcript.

## Acceptance
- Fresh open uses route resolution followed by one bounded SSE init for metadata and newest messages.
- No initial `/messages/latest`, `init_mode`, or `after_message_floor` request.
- Init coverage is exact for empty, short, exactly-limit, long, aligned, and oversized-render-unit cases.
- Archived and provisioning conversations initialize correctly from SSE.
- Reconnect uses only the event cursor and does not duplicate or lose messages/events.
- Scroll/deep-link history remains lazy and generation guarded.
- Focused Rust/UI/Allium tests, codegen, full `./dev.py check`, browser journey, and independent review pass.

## Non-goals
- No transcript IDB removal in this slice.
- No sidebar snapshot or operation-journal work.
- No offline transcript support.
