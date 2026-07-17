# Project Instruction Snapshots — Executive

## Status

Phoenix persists normalized project-guidance and skill bundles—including each exact frontmatter-stripped `SKILL.md` body, argument hint, and stable invocation paths—and uses one active bundle for model requests, prompt inspection, slash expansion, and LLM Skill-tool invocation. Spawned sub-agents receive a new active bundle copied exactly from the parent's persisted active bundle before runtime startup, and creation workers atomically commit finalized metadata plus the initial bundle under their fenced claim. A side-effect-free status API computes current-source drift without replacing the persisted candidate; an explicit preview API persists the exact review candidate, and confirmation remains exact-ID. Direct and idle-drained turns bind expansion to an exact queued bundle ID; compare-and-swap activation atomically swaps that bundle, bumps transcript generation, and persists a content-free System timeline event before sequenced SSE emission, while queue races reject the turn with a gap-free sequenced resynchronization error. Mid-turn steering drains do not activate. The transcript header shows current, changed, queued, and queued-with-newer-source states; users explicitly check, review a content-free manifest and estimated cache impact, then confirm the exact candidate.

## Intended surfaces

- Immutable normalized guidance and exact primary skill-body bundles per conversation; companion files remain live
- Explicit refresh preview with source manifest and cache-rewarm estimate
- Exact candidate confirmation and queued activation at the next user-turn boundary
- Visible stale, queued, and activated states in the conversation UI
- Durable recovery and provider-continuation invalidation

## Verification

Domain, database, API, runtime, and UI tests cover exact parent-to-child bundle copying across filesystem mutation and restart, stale creation-claim rejection with successor-bundle precedence, captured skill-body hashing and persistence, mutation-stable slash and Skill-tool invocation, manifest statuses, content-free serialization, exact candidate confirmation, status reads preserving candidates, queued-bundle preservation across later source changes, persisted prompt inspection after filesystem mutation, lazy legacy initialization, durable activation-message recovery, direct and idle-drained turn-boundary activation, mid-turn non-activation, stale previews, request races, focus restoration, and queued-with-newer-source rendering.

A live GPT-5.6 Sol verification initialized a snapshot, changed `AGENTS.md`, reviewed and queued the exact candidate, and activated it on the next user turn. The activation advanced transcript generation once and persisted the visible boundary before the user message. The first post-refresh turn used 2,184 uncached and 5,888 cached input tokens. After another unreviewed `AGENTS.md` mutation, the next turn used 163 uncached and 7,936 cached input tokens; prompt inspection still contained the reviewed snapshot and excluded the later source text. These account-specific values are evidence of the boundary and restored warm-cache behavior, not provider guarantees.
