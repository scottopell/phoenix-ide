# Project Instruction Snapshots — Executive

## Status

Phoenix persists normalized project-guidance and skill-catalog bundles and uses one active bundle for model requests and prompt inspection. A side-effect-free status API computes current-source drift without replacing the persisted candidate; an explicit preview API persists the exact review candidate, and confirmation remains exact-ID. Activation at direct and idle-drained user-turn boundaries atomically swaps roles, bumps transcript generation, and persists a content-free System timeline event before sequenced SSE emission. Mid-turn steering drains do not activate. The transcript header shows current, changed, queued, and queued-with-newer-source states; users explicitly check, review a content-free manifest and estimated cache impact, then confirm the exact candidate.

## Intended surfaces

- Immutable normalized guidance and skill-catalog bundles per conversation
- Explicit refresh preview with source manifest and cache-rewarm estimate
- Exact candidate confirmation and queued activation at the next user-turn boundary
- Visible stale, queued, and activated states in the conversation UI
- Durable recovery and provider-continuation invalidation

## Verification

Domain, database, API, runtime, and UI tests cover manifest statuses, content-free serialization, exact candidate confirmation, status reads preserving candidates, queued-bundle preservation across later source changes, persisted prompt inspection after filesystem mutation, lazy legacy initialization, durable activation-message recovery, direct and idle-drained turn-boundary activation, mid-turn non-activation, stale previews, request races, focus restoration, and queued-with-newer-source rendering.

A live GPT-5.6 Sol verification initialized a snapshot, changed `AGENTS.md`, reviewed and queued the exact candidate, and activated it on the next user turn. The activation advanced transcript generation once and persisted the visible boundary before the user message. The first post-refresh turn used 2,184 uncached and 5,888 cached input tokens. After another unreviewed `AGENTS.md` mutation, the next turn used 163 uncached and 7,936 cached input tokens; prompt inspection still contained the reviewed snapshot and excluded the later source text. These account-specific values are evidence of the boundary and restored warm-cache behavior, not provider guarantees.
