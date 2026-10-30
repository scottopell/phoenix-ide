# Durable Conversation Creation — Executive Summary

## Purpose

Slack gateway REQ-SG-008 adds a trusted Direct/no-repository creation intent, including when the server home is Git-backed (ADR-148). The implementation has focused fixture coverage; production acceptance is tracked in `specs/slack-gateway/executive.md` and does not change the verification status of other creation paths below.

Conversation creation accepts one durable job per request before filesystem, Git, attachment, or runtime work. Git-backed creation records one immutable starting pin and publishes the conversation only when its starting state is whole. Provisioning is governed by an exclusively claimed, restart-safe protocol whose behavior can be exercised under deterministic generated schedules.

## Current Reality

The Coordinator-only native creation adapter (REQ-CCR-013, ADR-191) shares public durable acceptance and records internal source-call provenance. Its objective is delivered literally as agent content. Readback derives current Coordinator authority, job, execution evidence and publication aggregate from one read transaction; both native read and submit receipts use that projection. Historical source `8d6ee3aafcad` passed scoped Clippy, DB25, IDE18, acceptance7 and all-target compilation on the sole qualification host. Its hosted jobs passed on a synthetic merge checkout, not the plain source pin. Exact-source task validation passed; the full compiler-bearing spec-shape/devpy lane timed out and is not qualified.

An accepted deletion finding supersedes that readiness: publication-target foreign keys cascaded away creation jobs and allowed request-identity reuse after deletion. Additive migration147 retains the authoritative job and both publication identities, preserving actual provenance, immutable pins and child images/reservations. Native historical publications have no live route; delivery mutation sinks and scheduling exclude a missing original transcript, even if a successor keeps the product alive. Registered native journey, held-claim/deadline, successor-only deletion, terminal-evidence and registered migration rollback/data-preservation regressions are authored. These corrections await serialized exact-successor qualification; historical passes do not qualify them. Already-deleted receipts are not reconstructed.

Exact successor `c7d51987f59b` passed strict scoped Clippy on the sole host, then failed the DB snapshot regression at the pending-publication assertion. The retention projection had misclassified an existing hidden `delivery_pending` original as historical because the ordinary-product reader intentionally hides unpublished aggregates. The correction preserves the publication-status gate while retaining history when the original is actually missing; the existing snapshot regression is unchanged, and the held-delivery deletion regression now asserts pending-hidden versus deleted-original readback. The failing sequence stopped before IDE, acceptance, task validation and the named devpy diagnostic. The corrected source requires new exact-source execution; neither partial DB successes nor Clippy at the failed pin qualify it.

Ordinary web creation enters through a directory-first typed ProductConversation request and provisions Git-backed conversations in detached-default worktrees without Project, mode, or branch inputs. The implementation contains request-bound durable acceptance, claim-fenced provisioning checkpoints, and a staged delivery lifecycle: `commit_metadata` atomically stages the aggregate as hidden `delivery_pending` at `bootstrap_initial_turn`; durable objective acceptance precedes ready publication (`published` in persistence) at `finalize_publication`. Delivery claims renew during asynchronous work. Delivery failures retain the same aggregate and objective identity for bounded automatic retry or eligible explicit retry; they do not offer creation Delete or Start over. Exact-request browser recovery stores complete intent in IndexedDB, activates a tab-scoped pointer only after durable storage, and navigates only after published observation.

Cancellation includes `delivery_pending` only before durable objective acceptance. The enqueue sink and cancellation share persistence write authority: cancellation-first fences enqueue; receipt-first excludes cancellation even after queue drain. Cleanup removes only an exactly owned hidden staged ProductConversation/transcript/WorkScope aggregate and safely reconciled owned resources, preserving unowned working-directory segments. The exact-request recovery surface remains available while normal conversation routes and lists hide staged work. Its advertised pending Cancel action is revalidated by the cancellation transaction rather than granting authority after acceptance.

The Allium model records these delivery states, optional whole claim fields, heartbeat fencing, acceptance receipts, cancellation linearization, and identity-preserving retry. The separate pure protocol vocabulary and generated operation-sequence tests remain present (`crates/phoenix-state-machine/src/creation_protocol.rs`, `transition.rs`); their coverage must not be treated as qualification of the production delivery path. Hidden `GitRepository` identity and `WorkScope.repository` attachment remain separate normative targets; under ADR-035 and REQ-GITREP-009, legacy `Project` remains the live repository authority.

Ordinary creation with omitted model uses the registry product default independently of workspace mode; explicit model/effort/speed selections remain unchanged (REQ-CCR-011). Cheap auxiliary work retains its separate selector.

## Verification Coverage

| Requirement | Status | Verification |
| --- | --- | --- |
| REQ-CCR-011 Ordinary Creation Model Default | Implemented | Registry preference and creation model-resolution tests; browser remembered-selection regression |
| REQ-CCR-001 Durable Acceptance | Implemented; qualified | Request-bound HTTP 202 acceptance/replay, exact-request status, browser uncertain-response replay, and published-only navigation regressions. |
| REQ-CCR-002 Exclusive Authority | Implemented; qualified | Allium claim rules plus live heartbeat, replaced-claim, cancellation, and lease-expiry worker regressions. |
| REQ-CCR-003 Crash Reconciliation | Implemented; qualified | Startup and periodic durable reconciliation reclaim accepted or expired work without relying on process-local wakes. |
| REQ-CCR-004 Bounded Retry | Implemented; qualified | Queue, objective-expansion, automatic-budget exhaustion, and eligible explicit-delivery retry regressions preserve aggregate identity. |
| REQ-CCR-005 Repository Serialization | Implemented; qualified | Bounded repository mutation locking and ownership-safe ambiguity retention are covered by lock-timeout and cleanup tests. |
| REQ-CCR-005A Immutable Starting Pin Selection | Implemented; qualified | Pin persistence, detached exact-OID worktree materialization, and no commit/tag/local-branch mutations are covered by behavioral and source-contract checks. |
| REQ-CCR-006 Runtime Bootstrap | Implemented; qualified | Claim-fenced objective expansion/enqueue, durable receipt replay, and exact-fingerprint completion avoid duplicate acceptance. |
| REQ-CCR-006A Atomic Publication | Implemented; qualified | Hidden aggregate staging and `complete_product_creation_delivery` publish identities atomically after durable objective acceptance. |
| REQ-CCR-007 Cancellation | Implemented; qualified | Cancellation and enqueue share transactional authority; cancel-first/enqueue-first, heartbeat, and owned-aggregate cleanup regressions cover both linearizations. |
| REQ-CCR-008 Deletion | Implemented; qualified | Hidden aggregate deletion and ownership ambiguity retention are verified; cleanup ambiguity remains visible for retry or local-only browser release without unsafe deletion. |
| REQ-CCR-009 Durable Scheduling | Implemented; qualified | Immediate kicks provide latency while periodic reconciliation and persisted retry deadlines provide correctness after lost wakes. |
| REQ-CCR-010 Deterministic Verification | Implemented; qualified | Generated protocol schedules, focused persistence/worker/UI tests, and ProductConversation E2E exact-request polling run in hosted checks. |
| REQ-CCR-012 Provisioning-Stage Migration | Implemented; qualified | `MIGRATION_128` backfill/constraint regression and exact current-schema 126/127/128 qualification run in hosted checks. Policy: ADR-104 and ADR-105. |
| REQ-CCR-013 Coordinator Intent and Provenance | Implemented; successor qualification pending | Snapshot authority/readback, literal intent, immutable UUID replay, retained historical publication, deletion fences and migration147 regressions; no final readiness credit from historical source. |

Hosted qualification covers Rust, cargo-fmt, Clippy, UI/spec, task, E2E, musl production-feature, and macOS helper suites; its checkout identity must be reconciled separately from exact-source receipts. The changed conversation-creation Allium model parses with zero error-severity diagnostics. Static review does not replace compiler or runtime qualification; the exact-source DB run exposed the pending-publication regression described above.

## Provisioning Checkpoint Follow-Up

`Database::advance_product_creation_provisioning_stage` and the product worker checkpoint helper enforce exact-next-stage, live-claim fencing, monotonic stage timestamps, retry floors, and direct-path no-op stage traversal. Publication and delivery completion require their immediate predecessor stages. Database, worker, migration, browser, and E2E regressions are part of exact-head hosted qualification.

## Rehomed Project Requirements

REQ-PROJ-000, REQ-PROJ-001, REQ-PROJ-002, REQ-PROJ-005, REQ-PROJ-005A, REQ-PROJ-017, REQ-PROJ-022, REQ-PROJ-028, and REQ-PROJ-029 retain their immutable IDs in this spec. Their unified Git-backed creation behavior is partially implemented by the ordinary web creation path; legacy compatibility creation remains until its separately tracked retirement.

## Merge Gate

Coordinator create/rename readiness remains on hold until the deletion-retention successor passes actual exact-source qualification, the full spec-shape/devpy timeout is resolved or explicitly dispositioned, and hosted checks and review findings are reconciled at the new head. Final human PR998 merge approval remains required. No source, test, or review result authorizes merge or deployment.
