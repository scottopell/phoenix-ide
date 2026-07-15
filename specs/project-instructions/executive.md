# Project Instruction Snapshots — Executive

## Status

Phoenix persists normalized project-guidance and skill-catalog bundles and uses one active bundle for model requests and prompt inspection. A side-effect-free status API computes current-source drift without replacing the persisted candidate; an explicit preview API persists the exact review candidate, and confirmation remains exact-ID. Activation at direct and idle-drained user-turn boundaries atomically swaps roles, bumps transcript generation, and persists a content-free System timeline event before sequenced SSE emission. Mid-turn steering drains do not activate. The UI surface remains pending.

## Intended surfaces

- Immutable normalized guidance and skill-catalog bundles per conversation
- Explicit refresh preview with source manifest and cache-rewarm estimate
- Exact candidate confirmation and queued activation at the next user-turn boundary
- Visible stale, queued, and activated states in the conversation UI
- Durable recovery and provider-continuation invalidation

## Verification

Domain, database, API, and runtime tests cover manifest statuses, content-free serialization, exact candidate confirmation, status reads preserving candidates, queued-bundle preservation across later source changes, persisted prompt inspection after filesystem mutation, lazy legacy initialization, durable activation-message recovery, and direct turn-boundary activation before the user message without mid-turn activation. UI coverage and repeated live cache measurements remain pending; the baseline is recorded in `docs/research/codex-token-efficiency-hunt.md`.
