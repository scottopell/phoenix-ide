# ADR-129: Ordinary skill catalog advertisement is executor-turn-local

- **Status:** Accepted
- **Date:** 2026-10-18
- **Affects:** REQ-SK-007, REQ-SK-008, REQ-SK-009

## Context

Ordinary request construction rediscovers skill metadata between model/tool calls.
Filesystem edits can therefore rewrite the system prefix inside one accepted turn.
The approved bounded scope requires refreshing at each new turn, including automatic
goal continuation, while invocation reads current bodies. There is no admitted
provider-universal authoritative appended-catalog protocol or durable snapshot store.

## Options considered

1. Rediscover on every model call: live advertisements, but unstable turn prefixes.
2. Retain metadata for one executor turn: bounded state, stable advertisements,
   with current invocation discovery and explicit restart limits.
3. Persist metadata and bind invocation identity/body versions: stronger historical
   parity, but new storage and invocation semantics outside the approved scope.

## Decision

Choose option 2. Resolve metadata lazily at the first dispatch after an accepted
new-turn boundary. Ordinary user input, approval feedback, and question answers
invalidate the prior catalog only after both input and resulting state persist.
Committed authoritative input and committed Idle steering starts also invalidate
it. Rejected input, failed persistence, and stale authoritative replay retain it. Model/tool retries and mid-turn
steering do not. Store only rendered advertisement text, not bodies or execution
authority. The Coordinator retains its authenticated embedded catalog unchanged.

Invocation continues to resolve current names and read current bodies. A name is
not an immutable identity: newly shadowing skills and fallback after removal follow
existing precedence, and the returned base-directory header identifies the actual
winner. This deliberately preserves current semantics rather than silently adding
path pinning. Advertising metadata is not a permission grant.

## Consequences

- Metadata edit/add/remove cannot rewrite an ordinary catalog within a live turn.
- Each new turn may rebuild the system prefix and incur a cache miss. No synthetic
  provider update item is introduced, and other guidance remains outside this scope.
- Reconstructed executors discover afresh, including when resuming an interrupted
  turn. There is no persisted or cross-version snapshot guarantee.
- Advertised metadata may differ from the current invocation source and body.
- Automatic-goal producers must enter through a new-turn admission boundary, not
  a raw model re-dispatch. This revision has no such producer to qualify.

## References

- `specs/skills/requirements.md`, `specs/skills/skills.allium`
- `OrdinarySkillCatalog`, `ConversationRuntime::dispatch_llm_request`
- ADR-034 compatibility policy
