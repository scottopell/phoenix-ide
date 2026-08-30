# ADR-040: Close uses WorkScope gates and tmux-only durable identity

- **Status:** Accepted
- **Date:** 2026-08-26
- **Affects:** REQ-WL-002b, REQ-WL-002d, REQ-PROJ-WS-001
- **Supersedes:** ADR-039's universal resource-instance persistence; ADR-026's no-global-one-to-one consequence

## Context

Product lifecycle, transcript topology, and resource ownership remain distinct facts. That distinction does not require several ordinary ProductConversations to own the same WorkScope, nor does it require every live resource to have durable cross-restart identity.

Close needs one reliable destructive boundary: an owning ordinary ProductConversation seals its WorkScope admission gate, stops resources known to that live Phoenix process, and removes a worktree only after current loss inspection and confirmation. A crash breaks process-local authority. Persisting identities for bash, PTY, and browser resources makes their ordinary process-epoch lifetime appear durable while requiring broad locator, process, and profile recovery machinery that Close does not need. Tmux is different: it intentionally survives Phoenix restart and provides a Phoenix-controlled server token at its socket endpoint.

## Options considered

1. Retain normalized durable identity and individual receipts for every runtime resource.
2. Treat all resources as process-epoch and never resume runtime cleanup after restart.
3. Seal one WorkScope gate, retain durable tmux identity and worktree outcomes, and conservatively leave process-epoch leftovers after restart.

## Decision

Choose option 3.

An ordinary ProductConversation is the sole owner of each WorkScope. Continuation rows and subordinate executions are members of that owning aggregate and may use its scope without becoming owners. A second ordinary ProductConversation cannot attach to that scope. Legacy records that contradict this relation are repair input and do not grant destructive authority.

Close seals one WorkScope admission gate before stopping resources held by the current Phoenix process epoch. Bash, PTY, browser, and equivalent ordinary execution resources have no universal durable identity or per-resource Close receipt. After a restart, Phoenix does not discover or signal those former-epoch resources. It may leave leftovers.

Tmux is intentionally process-persistent. Close seals its socket path and Phoenix-controlled server token as the durable identity, records durable tmux and worktree outcomes, and resumes only when that identity proves the same server. A socket mismatch, missing token, inaccessible probe, ownership conflict, changed worktree identity, or unconfirmed user changes produces NeedsRepair and leaves the resource or worktree untouched.

## Consequences

- **Positive:** Close has one visible resource-admission boundary and no universal resource lifecycle database.
- **Positive:** Restart recovery remains safe for tmux and worktrees without treating PID, process-group, browser-profile, or path observations as authority.
- **Positive:** A WorkScope cannot have competing ordinary product owners in steady state.
- **Negative:** A crash may leave bash, PTY, browser, and equivalent process-epoch resources behind for manual cleanup.
- **Negative:** Legacy scope-owner conflicts require explicit repair before destructive operations.

## References

- ADR-026: product lifecycle, transcript topology, and resource ownership are distinct
- ADR-039: prior universal durable resource-instance decision superseded here
- `specs/work-lifecycle/requirements.md`
- `specs/tmux-integration/requirements.md`
- `TmuxRegistry::rehydrate_retirement`
