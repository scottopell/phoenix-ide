# Work Lifecycle — Executive Summary

## What This Spec Covers

The work lifecycle spec now describes the intended user-facing **Close conversation** flow for Git-backed conversations, the worktree-loss inspection it requires, and the idempotent retirement of attached `WorkScope` resources without branch or PR mutation.

## Current Reality

That unified lifecycle is not yet the shipped product behavior. Phoenix still exposes legacy terminal actions instead of Close: `POST /api/conversations/:id/abandon-task` and `POST /api/conversations/:id/mark-merged` are live, and both continue to gate on the current conversation row plus `continued_in_conv_id` compatibility checks in `crates/phoenix-ide/src/api/lifecycle_handlers.rs`. Archive is still a separate ordinary API action (`POST /api/conversations/:id/archive`) backed by `archived` row state rather than the future Open/History aggregate transition. Legacy cleanup logic still captures abandon diff state and still distinguishes branch-disposition by legacy mode/worktree semantics.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-WL-001 | Close conversation is the only intended user-facing terminal lifecycle action for Git-backed conversations |
| REQ-WL-002 | Retirement inspection classifies exact worktree-loss risk before destructive teardown |
| REQ-WL-002a | Discard confirmation binds to one exact inspected workspace generation |
| REQ-WL-002b | Retirement retires owned resources stepwise, idempotently, and without automatic recovery artifacts |
| REQ-WL-003 | Pull-request state guides Close but never triggers it |

## Normative Authority

Current normative authority is `requirements.md`, `specs/bedrock/bedrock.allium`, and ADR-026. This executive intentionally reports current implementation drift instead of treating the normative Close model as shipped.

## Implementation Status

| Requirement | Status | Surface |
|-------------|--------|---------|
| REQ-WL-001 | Not implemented | Shipped UX still uses legacy abandon / mark-merged endpoints instead of one Close action |
| REQ-WL-002 | Partially implemented | Legacy flows already inspect/capture worktree state for cleanup paths, but the exact Close loss-inventory contract is not the shipped user flow |
| REQ-WL-002a | Not implemented | No shipped fingerprint-bound discard confirmation for the unified Close obligation |
| REQ-WL-002b | Partially implemented | Cleanup of worktree-owned resources exists, but it is still entered through legacy archive / abandon / mark-merged / hard-delete paths rather than a single durable Close obligation |
| REQ-WL-003 | Partially implemented | Observed PR state already guides current cleanup affordances, but it still participates in legacy mark-merged UX rather than purely advisory Close guidance |

## Legacy Surface Inventory

The following legacy surfaces are still current reality and must remain called out as such until code changes land:

- `/abandon-task` — shipped destructive terminal flow with diff capture and mode-dependent cleanup
- `/mark-merged` — shipped cleanup flow keyed to current branch/PR completion UX
- `/archive` — shipped archived/non-archived lifecycle split, separate from Close
- continuation gating via `continued_in_conv_id` — shipped protection against closing/cleaning up predecessors after handoff

## Validation Notes

Current-reality verification for this reconciliation used:

- `crates/phoenix-ide/src/api/lifecycle_handlers.rs`
- `crates/phoenix-ide/src/api/handlers.rs`
- `crates/phoenix-db/src/lib.rs` (`archive_conversation`, `archived` listings, continuation/ownership queries)

## Provenance

This spec supersedes the older abandon / mark-merged design at the normative layer, but the implementation has not been reconciled yet. Executives must therefore distinguish intended authority from shipped behavior rather than marking the unified lifecycle complete early.
