# Work Lifecycle — Executive Summary

## What This Spec Covers

The work lifecycle spec now describes the intended user-facing **Close conversation** flow for Git-backed conversations, the worktree-loss inspection it requires, the immutable restart-repair evidence retained when a registered worktree is missing or inaccessible after restart, and the idempotent retirement of attached `WorkScope` resources without branch or PR mutation.

## Current Reality

Durable Close retirement and ProductConversation History finalization are shipped, while the dedicated Close-start replacement and legacy-edge removal remain incomplete. A successful exact attempt retires the attached WorkScope resources, records one durable outcome message, transitions the ordinary aggregate to History in the completion transaction, and publishes compatibility updates only after commit. Primary ProductConversation surfaces expose Close and read-only History rather than Archive, but `POST /api/conversations/:id/archive`, `/abandon-task`, `/mark-merged`, and `continued_in_conv_id` compatibility checks remain live internally or on legacy surfaces. Existing row-level WorkScope fields remain attachment authority, with no parallel writable normalized attachment relation. Phoenix continues using the current Project-backed repository model; replacement is deferred until a named feature requires it. Exact-attempt adoption of immutable restart-repair evidence remains incomplete.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-WL-001 | Close conversation is the only intended user-facing terminal lifecycle action for Git-backed conversations |
| REQ-WL-002 | Retirement inspection classifies exact worktree-loss risk before destructive teardown |
| REQ-WL-002a | Discard confirmation binds to one exact inspected workspace generation |
| REQ-WL-002b | Retirement retires owned resources stepwise, idempotently, and without automatic recovery artifacts |
| REQ-PROJ-028a | Restart retains immutable repair evidence and fail-closed adoption for missing/inaccessible worktrees |
| REQ-WL-003 | Pull-request state guides Close but never triggers it |

## Normative Authority

Current normative authority is `requirements.md`, `work-lifecycle.allium`, `specs/bedrock/bedrock.allium`, and the restart-repair evidence defined in `specs/git-repository/git-repository.allium`. ADR-026 records WorkScope resource ownership; ADR-031 records staged single authority for ProductConversation lifecycle and attachment persistence; ADR-032 records the hidden-repository identity plus retained repair-evidence adoption rules. This executive intentionally reports current implementation drift instead of treating the normative Close model as shipped.

## Implementation Status

| Requirement | Status | Surface |
|-------------|--------|---------|
| REQ-WL-001 | Partially implemented | Primary ProductConversation surfaces expose Close, but legacy abandon / mark-merged endpoints and dedicated Close-start replacement remain incomplete |
| REQ-WL-002 | Partially implemented | Legacy flows already inspect/capture worktree state for cleanup paths, but the exact Close loss-inventory contract is not the shipped user flow |
| REQ-WL-002a | Not implemented | No shipped fingerprint-bound discard confirmation for the unified Close obligation |
| REQ-WL-002b | Partially implemented | Durable Close retirement idempotently retires attached WorkScope resources and completes with one outcome plus aggregate History; legacy entry and cleanup edges remain |
| REQ-PROJ-028a | Not implemented | Missing/inaccessible registered worktrees are not yet preserved as immutable restart-repair evidence that later Close attempts can adopt fail-closed by exact identity |
| REQ-WL-003 | Partially implemented | Observed PR state already guides current cleanup affordances, but it still participates in legacy mark-merged UX rather than purely advisory Close guidance |

## Legacy Surface Inventory

The following legacy surfaces are still current reality and must remain called out as such until code changes land:

- `/abandon-task` — shipped destructive terminal flow with diff capture and mode-dependent cleanup
- `/mark-merged` — shipped cleanup flow keyed to current branch/PR completion UX
- `/archive` — shipped compatibility entry used internally by the current Close journey and still reachable from legacy surfaces; aggregate lifecycle authority is Open/History
- continuation gating via `continued_in_conv_id` — shipped protection against closing/cleaning up predecessors after handoff

## Validation Notes

Current-reality verification for this reconciliation used:

- `crates/phoenix-ide/src/api/lifecycle_handlers.rs`
- `crates/phoenix-ide/src/api/handlers.rs`
- `crates/phoenix-db/src/lib.rs` (`archive_conversation`, `archived` listings, continuation/ownership queries)

## Provenance

This spec supersedes the older abandon / mark-merged design at the normative layer, but the implementation has not been reconciled yet. Executives must therefore distinguish intended authority from shipped behavior rather than marking the unified lifecycle complete early.
