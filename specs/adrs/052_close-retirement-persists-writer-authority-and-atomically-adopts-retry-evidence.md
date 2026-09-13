# ADR-052: Close retirement persists writer authority and atomically adopts retry evidence

- **Status:** Accepted
- **Date:** 2026-09-13
- **Affects:** REQ-WL-002b, REQ-WL-002c, REQ-BED-029, REQ-API-006; `AmbientWriterEvidenceRow`, `RetainedCleanupEvidenceAdoption`

## Context

Close retirement first stops resources owned through a sealed WorkScope gate, then quarantines and removes the worktree. Processes outside that gate may still hold filesystem references. A pathname reference alone does not prove write authority: read-only descriptors, current working directories, process roots, executable text, and read-only mappings may all name a path without being able to modify it. A positive decision also becomes unauditable if process identity and descriptor mode disappear before they are durably recorded.

Close retry introduces a second evidence boundary. Reinspection needs a fresh generation, while already-authorized dispatch and cleanup-plan evidence from the same exact attempt must remain usable. The relational schema correctly requires a cleanup plan to have a generation-matched dispatch parent. Rotating only the plan into a fresh generation creates an invalid parent/child combination; weakening that foreign key would instead allow cleanup without dispatch authority.

Recovery is user-facing. A raw storage-engine code cannot tell the client which Close attempt remains active, which transcript owns recovery, or which action is safe.

## Options considered

1. **Treat every path reference as a writer and retry a fixed number of times** — conservative about deletion, but read-only references become false positives, retry timing is unauditable, and vanished processes cannot be attributed.
2. **Remove generation coupling or reuse the prior inspection generation** — avoids a foreign-key failure, but weakens dispatch authorization or defeats fresh reinspection fencing.
3. **Persist typed writer authority and atomically adopt compatible retained evidence into each fresh retry generation** — preserves fail-closed deletion, fresh inspection truth, relational authorization, and auditable recovery at the cost of additional normalized evidence and a bounded observation protocol.

## Decision

Choose option 3.

An ambient positive writer is a complete typed observation: detector, process ID plus start/incarnation, executable, exact matched path, match kind, and writable descriptor access mode. Read-only descriptors and non-descriptor path relationships never confer write authority. Missing mandatory positive identity is detector-indeterminate and fails closed.

After owned resources retire, Phoenix performs no more than three ambient observations separated by 100 milliseconds on a monotonic clock. Final removal requires two consecutive authoritative no-writer observations. A transient writable incarnation may disappear, but that disappearance is accepted only after the two clean observations. A stable writer, a writer in the final observation, exhausted budget without two clean observations, or detector indeterminacy preserves quarantine and produces typed repair. Observation count, spacing, and clock are injectable for deterministic tests.

A retry keeps the same Close attempt and seals a fresh inspection generation. When compatible prior dispatch and cleanup-plan evidence exists, Phoenix validates exact attempt, WorkScope, worktree identity/fingerprint, typed locators, administrative-directory identity, sealed target inventory, and expected target resource. One transaction adopts the target-generation dispatch parent before its cleanup-plan child. Identical replay is idempotent, conflict rolls back both, and source generations remain immutable.

Persistence failures cross the domain boundary as stable invariant and relation identifiers, not raw SQLite codes. A Close compatibility request that encounters `needs_repair` either safely dispatches this same exact-attempt retry under aggregate mutation admission or returns typed recovery coordinates: attempt, active transcript, permitted retry action, and any failed invariant/relation.

## Consequences

- **Positive:** Read-only filesystem references cannot falsely authorize a writer decision; positive blocks are auditable after process exit; transient teardown can settle without weakening stable-writer protection.
- **Positive:** Fresh retry generations preserve relational dispatch authority and can converge idempotently through final WorkScope retirement.
- **Positive:** Clients receive exact recovery coordinates and stable invariant names instead of parsing prose or SQLite diagnostics.
- **Negative:** Process identity collection is platform-specific, and inability to collect complete identity remains a fail-closed repair condition.
- **Negative:** Retirement adds a bounded observation delay and normalized persistence rows.
- **Negative:** Retry adoption requires a larger transaction and explicit conflict classification.
- **Neutral:** Prior evidence generations and retained quarantine remain immutable; this decision does not authorize automatic retry of an existing production attempt, manual lifecycle-row repair, or deployment.

## References

- ADR-026, ADR-031, ADR-034, ADR-039, ADR-040, ADR-041, ADR-042
- `specs/work-lifecycle/requirements.md`
- `specs/work-lifecycle/work-lifecycle.allium`
- `specs/bedrock/requirements.md`
- `specs/bedrock/bedrock.allium`
- `specs/api/requirements.md`
- `macos_process_has_path_reference`
- `scan_macos_process_path_references`
- `retire_close_scope`
- `record_close_worktree_cleanup_plan`
