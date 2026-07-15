# Project Instruction Snapshots — Executive

## Status

Phoenix persists normalized project-guidance and skill-catalog bundles and uses one active bundle for model requests. Conversation-scoped refresh preview and exact-candidate confirmation APIs expose the content-free manifest, estimate labeling, and queued status. The UI surface remains pending.

## Intended surfaces

- Immutable normalized guidance and skill-catalog bundles per conversation
- Explicit refresh preview with source manifest and cache-rewarm estimate
- Exact candidate confirmation and queued activation at the next user-turn boundary
- Visible stale, queued, and activated states in the conversation UI
- Durable recovery and provider-continuation invalidation

## Verification

Domain, database, API, and runtime tests cover manifest statuses, content-free serialization, exact candidate confirmation, queued-bundle preservation across later source changes, lazy legacy initialization, and turn-boundary activation. UI coverage and repeated live cache measurements remain pending; the baseline is recorded in `docs/research/codex-token-efficiency-hunt.md`.
