# Durable Conversation Creation — Executive Summary

## Purpose

Conversation creation returns a durable shell before filesystem, Git, expansion, attachment, or runtime work. Provisioning is governed by an exclusively claimed, restart-safe protocol whose behavior can be exercised under deterministic generated schedules.

## Current Reality

The branch includes the shell-first API and UI flow. The durable protocol vocabulary, pure transition function, `CreationClaim`/`CompleteCreation` state-machine hooks, and deterministic operation-sequence tests are present (`crates/phoenix-state-machine/src/creation_protocol.rs`, `transition.rs`). Production persistence and worker orchestration still use the earlier unguarded phase-update model and remain to be migrated before the feature is mergeable. Git-backed creation also still enters through legacy Project/mode fields; hidden `GitRepository` identity, singular `WorkScope.repository` attachment, detached-default creation without a mode/branch picker, and typed unresolved default-branch evidence are normative targets rather than shipped authority.

## Verification Coverage

| Requirement | Status | Verification |
| --- | --- | --- |
| REQ-CCR-001 Durable Acceptance | Partial | Existing shell/job transaction; expensive acceptance work still needs migration |
| REQ-CCR-002 Exclusive Authority | Modelled | Pure claim/generation transition and stale-result tests |
| REQ-CCR-003 Crash Reconciliation | Modelled | Expired-claim takeover emits reconciliation rather than blind replay |
| REQ-CCR-004 Bounded Retry | Modelled | Pure four-attempt policy and generated operation schedules |
| REQ-CCR-005 Repository Serialization | Not implemented | Real Git reservation/locking tests required |
| REQ-CCR-006 Runtime Bootstrap | Partial | Initial-message creation checkpoints `Finalize`, then atomically commits its message, ready job status, and dispatchable runtime state under the current claim before provider dispatch. Stale authority mutates none of them, acknowledges a typed stale outcome, and retires the stale runtime before authoritative reconstruction. Retry-scheduled reconstruction remains non-dispatchable until current-claim settlement. Broader idempotent bootstrap and recovery remain incomplete |
| REQ-CCR-007 Cancellation | Modelled | Claim revocation and visible cancelled-state test |
| REQ-CCR-008 Deletion | Modelled | Hidden deletion-pending tombstone test |
| REQ-CCR-009 Durable Scheduling | Not implemented | Deadline-aware production scheduler required |
| REQ-CCR-010 Deterministic Verification | Partial | 512 generated protocol schedules plus checked-in minimized regressions |

## Rehomed Project Requirements

REQ-PROJ-000, REQ-PROJ-001, REQ-PROJ-002, REQ-PROJ-005, REQ-PROJ-005A, REQ-PROJ-017, REQ-PROJ-022, REQ-PROJ-028, and REQ-PROJ-029 retain their immutable IDs in this spec. Their unified Git-backed creation behavior is not fully implemented while legacy Project and mode surfaces remain reader/writer authority.

## Merge Gate

The async creation feature is not ready to merge until the production database, worker, Git/resource adapters, runtime bootstrap, and recovery UI conform to the normative protocol and pass deterministic, adapter, and end-to-end verification.
