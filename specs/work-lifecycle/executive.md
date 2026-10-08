# Work Lifecycle — Executive Summary

## What This Spec Covers

The work lifecycle spec now describes the intended user-facing **Close conversation** flow for Git-backed conversations, the worktree-loss inspection it requires, the immutable restart-repair evidence retained when a registered worktree is missing or inaccessible after restart, and the idempotent retirement of attached `WorkScope` resources without branch or PR mutation.

## Current Reality

### Bounded Close repair candidate

PR #865 adds exact same-attempt dispatch/cleanup-plan adoption, the supported FK-787 partial-generation recognizer, Linux all-process/all-task cwd inspection, and writer-safe final-tombstone recovery. These are candidate changes, not a claim that the full retirement implementation or retained production cleanup has shipped. Migration 119 preserves existing Close rows and installs lineage constraints without performing retry, adoption backfill, or filesystem deletion. No production retry, database surgery, quarantine deletion, deployment, or release is authorized by these artifacts.

`NeedsRepair` requires explicit retry; startup recovery remains permitted for an attempt already in `RetirementRequested`. Bedrock's retry guidance preserves that boundary, and aggregate completion requires `RetirementRequested`, not `NeedsRepair`. The retained-row recognizer requires the complete diagnostic `Database error: error returned from database: (code: 787) FOREIGN KEY constraint failed` together with every REQ-WL-005 row-shape condition. Existing broad lifecycle status below is retained rather than requalified by this candidate.

### Broader lifecycle status

Durable Close retirement and ProductConversation History finalization are shipped, while the dedicated Close-start replacement and legacy-edge removal remain incomplete. A successful exact attempt retires the attached WorkScope resources, records one durable outcome message, transitions the ordinary aggregate to History in the completion transaction, and publishes compatibility updates only after commit. Primary ProductConversation surfaces expose Close and read-only History rather than Archive, but `POST /api/conversations/:id/archive`, `/abandon-task`, `/mark-merged`, and `continued_in_conv_id` compatibility checks remain live internally or on legacy surfaces. Existing row-level WorkScope fields remain attachment authority, with no parallel writable normalized attachment relation. Phoenix continues using the current Project-backed repository model; replacement is deferred until a named feature requires it. Exact-attempt adoption of immutable restart-repair evidence remains incomplete.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-WL-001 | Close conversation is the only intended user-facing terminal lifecycle action for Git-backed conversations |
| REQ-WL-002 | Retirement inspection classifies exact worktree-loss risk before destructive teardown |
| REQ-WL-002a | Discard confirmation binds to one exact inspected workspace generation |
| REQ-WL-002b | Retirement retires owned resources stepwise, idempotently, and without automatic recovery artifacts |
| REQ-WL-002c | NeedsRepair requires explicit retry of the same attempt; already-requested retirement can recover at startup |
| REQ-WL-002d | Durable tmux identity requires a sealed socket plus Phoenix-controlled token |
| REQ-WL-004 | Fresh generations atomically adopt exact dispatch, complete cleanup plan and immutable lineage |
| REQ-WL-005 | Only the enumerated partial-generation foreign-key failure is supported by retained-pair recognition |
| REQ-WL-006 | Linux cwd inventory covers every process and task in the visible namespace and fails closed |
| REQ-WL-007 | Tombstone recovery freshly inspects writers, revalidates identity and handles exact crash/absence states |
| REQ-WL-008 | Migration 119 preserves existing evidence and performs no retirement or automatic retry |
| REQ-PROJ-028a | Restart retains immutable repair evidence and fail-closed adoption for missing/inaccessible worktrees |
| REQ-WL-003 | Pull-request state guides Close but never triggers it |

## Normative Authority

Current normative authority is `requirements.md`, `work-lifecycle.allium`, `specs/bedrock/bedrock.allium`, and the restart-repair evidence defined in `specs/git-repository/git-repository.allium`. ADR-026 records WorkScope resource ownership; ADR-031 records staged single authority for ProductConversation lifecycle and attachment persistence; ADR-032 records the hidden-repository identity plus retained repair-evidence adoption rules. This executive intentionally reports current implementation drift instead of treating the normative Close model as shipped. ADR-087 records the bounded same-attempt adoption and explicit retry policy; ADR-088 records all-task Linux cwd inventory and writer reinspection during tombstone crash recovery. Their compatibility boundary is feature-scoped under REQ-COMP-001/002 rather than a project-wide legacy recovery promise.

## Implementation Status

| Requirement | Status | Surface |
|-------------|--------|---------|
| REQ-WL-001 | Partially implemented | Primary ProductConversation surfaces expose Close, but legacy abandon / mark-merged endpoints and dedicated Close-start replacement remain incomplete |
| REQ-WL-002 | Partially implemented | Legacy flows already inspect/capture worktree state for cleanup paths, but the exact Close loss-inventory contract is not the shipped user flow |
| REQ-WL-002a | Not implemented | No shipped fingerprint-bound discard confirmation for the unified Close obligation |
| REQ-WL-002b | Partially implemented | Durable Close retirement idempotently retires attached WorkScope resources and completes with one outcome plus aggregate History; legacy entry and cleanup edges remain |
| REQ-WL-002c | Candidate; successor CI/review pending | Bedrock and Work lifecycle require explicit retry and retirement-only completion; hosted `a993112` evidence below does not qualify successor runtime guards |
| REQ-WL-002d | Existing contract; not requalified here | Sealed tmux socket/token authority remains outside the bounded adoption delta |
| REQ-WL-004 | Candidate; not qualified by this spec edit | `Database::adopt_close_worktree_cleanup_plan`; atomic rollback, repeated lineage adoption and completed-History hard-delete regressions required |
| REQ-WL-005 | Candidate; successor CI/review pending | `Database::resume_legacy_fk787_close_retirement_generation`; DB negative matrix covers exact-diagnostic near misses, zero captured worktrees, missing/extra inspections, missing/extra/unsealed active inventory, current dispatch/plan, wrong residuals and missing/mismatched prior authority; rejection asserts unchanged retained rows and obligation. Added matrix cases remain unexecuted pending successor CI |
| REQ-WL-006 | Candidate; Linux execution not performed here | `linux_namespace_cwd_scan`; non-leader/cross-UID tasks, unreadability, inventory churn, PID/TID reuse and exec identity regressions required |
| REQ-WL-007 | Candidate; not qualified by this spec edit | `resume_final_worktree_tombstone_with_writer_inspection`; persisted writer, indeterminate scan, replacement, pre-rename and unbound-object crash regressions required |
| REQ-WL-008 | Candidate; not qualified by this spec edit | `MIGRATION_119`; preservation and relational immutability constraints require migration regression execution |
| REQ-PROJ-028a | Not implemented | Missing/inaccessible registered worktrees are not yet preserved as immutable restart-repair evidence that later Close attempts can adopt fail-closed by exact identity |
| REQ-WL-003 | Partially implemented | Observed PR state already guides current cleanup affordances, but it still participates in legacy mark-merged UX rather than purely advisory Close guidance |

## Legacy Surface Inventory

The following legacy surfaces are still current reality and must remain called out as such until code changes land:

- `/abandon-task` — shipped destructive terminal flow with diff capture and mode-dependent cleanup
- `/mark-merged` — shipped cleanup flow keyed to current branch/PR completion UX
- `/archive` — shipped compatibility entry used internally by the current Close journey and still reachable from legacy surfaces; aggregate lifecycle authority is Open/History
- continuation gating via `continued_in_conv_id` — shipped protection against closing/cleaning up predecessors after handoff

## Validation Notes

### Hosted exact-head evidence and successor gate

[Hosted CI run 37746105326](https://github.com/scottopell/phoenix-ide/actions/runs/37746105326) completed successfully for exact head `a9931120638e3d42a3a8bfe1848cc681ae8a657a`: the e2e, clippy, ui/specs, rust, and task-validation check jobs were green. This supersedes the older predecessor-only evidence; it does not qualify the successor runtime changes or the added FK-787 negative matrix. Successor qualification remains pending exact-head hosted CI and independent review. Task 33001 remains in-progress until those gates are satisfied. Green CI is not release, deployment, or production cleanup authority.

### Bounded candidate specification validation

The negative-matrix/documentation pass reads `specs/AUTHORING.md`, the roadmap, hosted PR evidence, the REQ-WL-005 recognizer and migrated schema guards. It changes only DB tests and this executive, not the production recognizer. Validation is limited to rustfmt and lightweight specification checks; no Rust/UI build, Rust test execution, production operation or git action is part of this pass. Single-file Allium checking reports zero errors for Work lifecycle (3 warnings) and Bedrock (13 warnings), returning nonzero for structural warnings including imports outside the check set. The pure `dev.py` spEARS v2 shape validator reports zero errors. The earlier diagnostic-only SQLite smoke check is not execution evidence for the new Rust matrix; the added regressions remain unexecuted pending successor hosted CI and review.

Focused code regressions found in the candidate include `retry_generation_atomically_adopts_prior_dispatch_and_cleanup_plan`, `repeated_retry_adopts_newest_cleanup_plan_lineage_idempotently`, the `legacy_fk787_resume_*` tests, `migration_118_preserves_plans_and_enforces_exact_immutable_adoption`, `persisted_tombstone_writer_or_indeterminate_scan_blocks_deletion`, and the `cwd_scan_*` inventory/identity tests. Naming these is coverage inventory, not a claim that they were run by this specification pass.

The added DB cases are `legacy_fk787_resume_rejects_zero_captured_worktree_scopes`, `legacy_fk787_resume_rejects_inspection_count_mismatch` (missing and extra), `legacy_fk787_resume_rejects_missing_extra_or_unsealed_inventory`, and `legacy_fk787_resume_rejects_current_dispatch_or_cleanup_plan` (dispatch only, dispatch plus plan, and plan only). Each snapshots the full obligation and retained evidence tables after fixture mutation and requires a no-op rejection. Legacy shapes forbidden by current schema constraints are injected only in the in-memory test fixture; removed guards and foreign-key enforcement are restored before recognition.

### Broader reconciliation evidence

Current-reality verification for this reconciliation used:

- `crates/phoenix-ide/src/api/lifecycle_handlers.rs`
- `crates/phoenix-ide/src/api/handlers.rs`
- `crates/phoenix-db/src/lib.rs` (`archive_conversation`, `archived` listings, continuation/ownership queries)

## Provenance

Durable Close retirement and aggregate History finalization are shipped through compatibility entrypoints. Dedicated Close-start replacement and removal of abandon, mark-merged, archive, and continuation compatibility edges remain incomplete, so the unified lifecycle is only partially implemented.
