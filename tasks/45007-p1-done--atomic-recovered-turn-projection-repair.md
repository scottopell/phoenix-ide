# Atomically repair conversation projection when recovered direct-turn ownership terminates

## Production evidence

Conversation `fa54bbd8-205f-4c07-9d4c-3065b3ab95bd` exposed the failure after a deploy restart:

- 12:19:56 UTC: durable turn 32 was accepted/materialized and owned the conversation.
- 12:28:34 UTC: the last assistant message was persisted.
- 12:29:03 UTC: production restarted. `reset_all_to_idle` correctly preserved `llm_requesting` because the accepted materialized turn still had `owns_conversation = 1`.
- 17:26:04 UTC: `RuntimeManager::create_runtime` derived `initial_state = Idle` from durable/message evidence, then terminalized the recovered materialized turn as `Completed` and released ownership.
- The conversation projection was not repaired in that transaction, so `conversations.state` remained `llm_requesting` after no durable owner or active runtime work existed.
- Reconnect showed a stale “awaiting LLM” state and exposed Stop. The immediate Stop race is addressed separately by PR #607.

## Root cause

`RuntimeManager::create_runtime` handles a recovered `LoadedActiveDirectTurn { materialized: true }` whose derived `initial_state` is `Idle`, `HandedOff`, `Error`, or `ContextExhausted` by calling `RuntimeStorage::terminate_active_direct_turn(...Completed)`. `WorkflowRepository::terminate_authoritative_turn` atomically updates the direct-turn aggregate/effects, but does not update the conversation reducer projection. The previous projection can therefore retain a transient busy state that was intentionally preserved during startup while the turn owned the conversation.

A separate post-terminalization `update_conversation_state` is not sufficient: REQ-DWF-CHAT-012 permits persisted projections only when the same transaction changes their authority, and a second write recreates a crash window.

## Owning invariant

When recovery determines that a materialized direct turn no longer represents active reducer work, releasing that turn's durable conversation ownership and repairing the persisted reducer projection to the already-derived safe state must be one typed, generation-checked SQLite transaction. A stale/replayed authority must not overwrite a newer conversation state or a newer owning turn.

## Starting code anchors

- `RuntimeManager::create_runtime` recovered materialized-turn branch in `crates/phoenix-ide/src/runtime.rs`
- `RuntimeStorage::terminate_active_direct_turn` and `DatabaseRuntimeStorage` in `crates/phoenix-ide/src/runtime/traits.rs`
- `WorkflowRepository::terminate_authoritative_turn` in `crates/phoenix-db/src/workflow/direct_turn.rs`
- `Database::reset_all_to_idle` in `crates/phoenix-db/src/lib.rs`
- REQ-DWF-CHAT-012 through REQ-DWF-CHAT-014 in `specs/durable-workflows/requirements.md`

## Required design

- Add a typed recovery terminalization input/result rather than passing independent turn and conversation writes by convention.
- Generation/CAS-check the exact recovered turn authority.
- Atomically terminalize the direct turn, release `owns_conversation`, suppress/interrupt nonterminal child effects, and update `conversations.state` to the already-derived recovery state.
- Refuse/no-op stale authority if a newer turn owns the conversation or the conversation projection has advanced incompatibly.
- Keep ordinary executor terminalization semantics distinct unless the same atomic projection obligation demonstrably applies there.

## Acceptance criteria

- [ ] Deterministic regression reproduces: persisted `llm_requesting` + materialized owning turn + derived recovery Idle; recovery leaves both turn terminal/non-owning and conversation projection Idle.
- [ ] Crash/failpoint tests before and after the transaction prove there is no state where ownership is released while the old transient projection remains committed.
- [ ] Stale generation/replay cannot overwrite a newer live owner or newer reducer state.
- [ ] Repeated recovery is idempotent.
- [ ] Runtime materialization does not resume an LLM for the terminalized stale turn.
- [ ] Reconnect/snapshot reads the repaired non-busy state.
- [ ] Requirements/Allium are updated if the exact recovery command changes normative behavior.

## Non-goals

- Do not broadly rewrite transient conversation state on every runtime materialization.
- Do not make DB projection an independent authority over the live reducer.
- Do not bundle `/pr-status` SQLite contention; that is a separate failure mode.
- Do not weaken the pure state machine by adding `Idle + UserCancel`.
