# Durable Conversation Creation — Executive Summary

## Purpose

Conversation creation accepts one durable job per request before filesystem, Git, attachment, or runtime work. Git-backed creation records one immutable starting pin and publishes the conversation only when its starting state is whole. Provisioning is governed by an exclusively claimed, restart-safe protocol whose behavior can be exercised under deterministic generated schedules.

## Current Reality

The branch includes the shell-first API and UI flow, but production persistence and worker orchestration still use earlier staged and phase-update behavior rather than the full simplified request-bound publication contract. The durable protocol vocabulary, pure transition function, `CreationClaim`/`CompleteCreation` state-machine hooks, and deterministic operation-sequence tests are present (`crates/phoenix-state-machine/src/creation_protocol.rs`, `transition.rs`). Ordinary web creation enters through a directory-first typed ProductConversation request and provisions Git-backed conversations in detached-default worktrees without Project, mode, or branch inputs. Hidden `GitRepository` identity, singular `WorkScope.repository` attachment, request-id job identity, immutable starting-pin persistence, atomic publication, and durable typed unresolved default-branch evidence remain normative targets rather than shipped authority.

## Verification Coverage

| Requirement | Status | Verification |
| --- | --- | --- |
| REQ-CCR-001 Durable Acceptance | Partial | Existing shell/job transaction exists, but request-id replay and no-partial-publication cutover remain incomplete |
| REQ-CCR-002 Exclusive Authority | Modelled | Pure claim/generation transition and stale-result tests; request-id-as-job-identity not fully implemented |
| REQ-CCR-003 Crash Reconciliation | Modelled | Expired-claim takeover emits reconciliation rather than blind replay; explicit cleanup ambiguity remains unimplemented |
| REQ-CCR-004 Bounded Retry | Modelled | Pure four-attempt policy and generated operation schedules |
| REQ-CCR-005 Repository Serialization | Not implemented | Real Git locking and ownership-safe ambiguous-cleanup handling required |
| REQ-CCR-005A Immutable Starting Pin Selection | Specified only | Origin-vs-no-origin pin persistence and pin immutability are not yet shipped |
| REQ-CCR-006 Runtime Bootstrap | Partial | Current claim-bound bootstrap work exists, but broader idempotent bootstrap and recovery remain incomplete |
| REQ-CCR-006A Atomic Publication | Specified only | Production still contains staged/early publication behavior rather than one atomic ready publication boundary |
| REQ-CCR-007 Cancellation | Modelled | Claim revocation and visible cancelled-state test |
| REQ-CCR-008 Deletion | Modelled | Hidden deletion-pending tombstone test; explicit ambiguity retention remains unimplemented |
| REQ-CCR-009 Durable Scheduling | Not implemented | Deadline-aware production scheduler required |
| REQ-CCR-010 Deterministic Verification | Partial | Generated protocol schedules exist, but immutable-pin and atomic-publication coverage still need implementation |

## Rehomed Project Requirements

REQ-PROJ-000, REQ-PROJ-001, REQ-PROJ-002, REQ-PROJ-005, REQ-PROJ-005A, REQ-PROJ-017, REQ-PROJ-022, REQ-PROJ-028, and REQ-PROJ-029 retain their immutable IDs in this spec. Their unified Git-backed creation behavior is partially implemented by the ordinary web creation path; legacy compatibility creation remains until its separately tracked retirement.

## Merge Gate

The async creation feature is not ready to merge until the production database, worker, Git/resource adapters, runtime bootstrap, and recovery UI conform to the normative protocol and pass deterministic, adapter, and end-to-end verification.
