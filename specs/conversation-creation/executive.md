# Durable Conversation Creation — Executive Summary

## Purpose

Conversation creation accepts one durable job per request before filesystem, Git, attachment, or runtime work. Git-backed creation records one immutable starting pin and publishes the conversation only when its starting state is whole. Provisioning is governed by an exclusively claimed, restart-safe protocol whose behavior can be exercised under deterministic generated schedules.

## Current Reality

Ordinary web creation enters through a directory-first typed ProductConversation request and provisions Git-backed conversations in detached-default worktrees without Project, mode, or branch inputs. The branch contains request-bound durable acceptance, claim-fenced provisioning checkpoints, and a staged delivery lifecycle: `commit_metadata` atomically stages the aggregate as hidden `delivery_pending` at `bootstrap_initial_turn`; durable objective acceptance precedes ready publication (`published` in persistence) at `finalize_publication`. Delivery claims renew during asynchronous work. Delivery failures retain the same aggregate and objective identity for bounded automatic retry or eligible explicit retry; they do not offer creation Delete or Start over. These authored changes are not a claim of shipped or end-to-end verified behavior.

Cancellation includes `delivery_pending` only before durable objective acceptance. The enqueue sink and cancellation share persistence write authority: cancellation-first fences enqueue; receipt-first excludes cancellation even after queue drain. Cleanup removes only an exactly owned hidden staged ProductConversation/transcript/WorkScope aggregate and safely reconciled owned resources, preserving unowned working-directory segments. The exact-request recovery surface remains available while normal conversation routes and lists hide staged work. Its advertised pending Cancel action is revalidated by the cancellation transaction rather than granting authority after acceptance.

The Allium model records these delivery states, optional whole claim fields, heartbeat fencing, acceptance receipts, cancellation linearization, and identity-preserving retry. The separate pure protocol vocabulary and generated operation-sequence tests remain present (`crates/phoenix-state-machine/src/creation_protocol.rs`, `transition.rs`); their coverage must not be treated as qualification of the production delivery path. Hidden `GitRepository` identity and `WorkScope.repository` attachment remain separate normative targets; under ADR-035 and REQ-GITREP-009, legacy `Project` remains the live repository authority.

Ordinary creation with omitted model uses the registry product default independently of workspace mode; explicit model/effort/speed selections remain unchanged (REQ-CCR-011). Cheap auxiliary work retains its separate selector.

## Verification Coverage

| Requirement | Status | Verification |
| --- | --- | --- |
| REQ-CCR-011 Ordinary Creation Model Default | Implemented | Registry preference and creation model-resolution tests; browser remembered-selection regression |
| REQ-CCR-001 Durable Acceptance | Authored; unvalidated | Request-bound acceptance/replay and hidden exact-request status path; handler `status_projection_covers_all_durable_states_without_premature_routes` asserts no premature route and delivery action sets. No test execution claimed. |
| REQ-CCR-002 Exclusive Authority | Authored; unvalidated | Allium `EligibleDeliveryClaimed`, `LiveClaimHeartbeatRenewsLease`, and `LostClaimHeartbeatStopsWork`; worker `delivery_claim_renewal_preserves_identity_and_expiry_fences`, `slow_delivery_keeps_short_test_lease_live_through_enqueue`, and replaced-claim heartbeat regression. |
| REQ-CCR-003 Crash Reconciliation | Modelled | Expired-claim takeover emits reconciliation rather than blind replay; explicit cleanup ambiguity remains unimplemented |
| REQ-CCR-004 Bounded Retry | Authored; unvalidated | Allium `DeliveryFailureSchedulesRetry`, `DeliveryRetryBudgetExhausted`, and `UserRetriesFailedDelivery`; worker `explicit_retry_after_queue_full_reuses_published_identities_without_duplicate_aggregate`; action tests cover retryable failure `[retry_delivery]` and ineligible failure `[]`. |
| REQ-CCR-005 Repository Serialization | Not implemented | Real Git locking and ownership-safe ambiguous-cleanup handling required |
| REQ-CCR-005A Immutable Starting Pin Selection | Specified only | Origin-vs-no-origin pin persistence and pin immutability are not yet shipped |
| REQ-CCR-006 Runtime Bootstrap | Authored; unvalidated | Allium `CurrentDeliveryClaimAcceptsObjectiveAtomically` and receipt replay; claim-fenced objective sink and exact-fingerprint completion avoid duplicate enqueue. |
| REQ-CCR-006A Atomic Publication | Authored; unvalidated | Allium `CurrentClaimStagesWholeAggregate` / `CurrentDeliveryClaimCompletesCreation`; persistence `publish_product_creation_atomically` / `complete_product_creation_delivery` separate hidden aggregate staging from ready exposure. |
| REQ-CCR-007 Cancellation | Authored; unvalidated | Allium `CreationCancellationRevokesAuthority`, `AcceptedObjectiveExcludesCreationCancellation`, and `CancelledHiddenAggregateRemoved`; DB `cancellation_before_delivery_fences_sink_and_renewal_then_removes_staged_aggregate`, `delivery_receipt_defeats_cancellation_even_after_queue_drain`, and `file_backed_cancellation_and_sink_have_exactly_one_winner`; worker `delivery_heartbeat_observes_committed_cancellation_and_drops_work`; handler/product action assertions require pending `[cancel]`. No tests run for this alignment. |
| REQ-CCR-008 Deletion | Modelled | Hidden deletion-pending tombstone test; explicit ambiguity retention remains unimplemented |
| REQ-CCR-009 Durable Scheduling | Not implemented | Deadline-aware production scheduler required |
| REQ-CCR-010 Deterministic Verification | Partial | Pure generated protocol schedules and focused delivery/cancellation adapter regressions are authored; execution and full production generated-schedule coverage remain unqualified. |
| REQ-CCR-012 Provisioning-Stage Migration | Authored; unvalidated | `MIGRATION_128` maps status, staging, and pin evidence and copies the row update timestamp; `migration_128_backfills_relational_stages_and_enforces_constraints` is the regression. Policy: ADR-104. No test execution claimed. |

The spec/action alignment was checked with the installed `allium check specs/conversation-creation/conversation-creation.allium`: zero errors and zero warnings (informational diagnostics only). No Rust tests, build, package-manager, or `dev.py` validation was run for this alignment; authored regression references above are not passing-test claims.

## Provisioning Checkpoint Follow-Up

`Database::advance_product_creation_provisioning_stage` and the product worker checkpoint helper have authored changes for exact-next-stage, live-claim fencing, monotonic stage timestamps, retry floors, and direct-path no-op stage traversal. Publication and delivery completion require their immediate predecessor stages. Focused database and worker regressions have been added or updated, but have not been run. This does not change the broader merge gate or establish runtime, migration, or end-to-end acceptance.

## Rehomed Project Requirements

REQ-PROJ-000, REQ-PROJ-001, REQ-PROJ-002, REQ-PROJ-005, REQ-PROJ-005A, REQ-PROJ-017, REQ-PROJ-022, REQ-PROJ-028, and REQ-PROJ-029 retain their immutable IDs in this spec. Their unified Git-backed creation behavior is partially implemented by the ordinary web creation path; legacy compatibility creation remains until its separately tracked retirement.

## Merge Gate

The async creation feature is not ready to merge until the production database, worker, Git/resource adapters, runtime bootstrap, and recovery UI conform to the normative protocol and pass deterministic, adapter, and end-to-end verification.
