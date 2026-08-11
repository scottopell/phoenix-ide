# Make accepted steering messages live across clients

## Problem
A steering message accepted by Phoenix is persisted server-side, but the live SSE event contains only its id and queue position. Only the browser that made the POST has the local content needed to render it. Other tabs and API/Coordinator sends remain invisible until drain; reconnect behavior depends on sender-local storage.

## Acceptance
- Server-accepted queued steering state is authoritative and renderable in every subscribed client.
- Same-tab optimistic messages reconcile without duplication.
- Cross-tab/API messages appear live and queued messages survive reconnect.
- Cancellation and drain remove/reconcile the queued bubble live.
- Audit sibling SSE handlers for the same sender-local or refresh-only assumption; fix on-path instances and capture bounded follow-ups.
- Rust wire/parity, frontend state, and real two-client journey coverage pass.
