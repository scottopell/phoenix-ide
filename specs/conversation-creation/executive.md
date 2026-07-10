# Durable Conversation Creation — Executive Summary

## Purpose

Conversation creation returns a durable shell before filesystem, Git, expansion, attachment, or runtime work. Provisioning is governed by an exclusively claimed, restart-safe protocol whose behavior can be exercised under deterministic generated schedules.

## Current Reality

The branch includes the shell-first API and UI flow. The durable protocol vocabulary, pure transition function, and initial deterministic operation-sequence tests are present. Production persistence and worker orchestration still use the earlier unguarded phase-update model and remain to be migrated before the feature is mergeable.

## Verification Coverage

| Requirement | Status | Verification |
| --- | --- | --- |
| REQ-CCR-001 Durable Acceptance | Partial | Existing shell/job transaction; expensive acceptance work still needs migration |
| REQ-CCR-002 Exclusive Authority | Modelled | Pure claim/generation transition and stale-result tests |
| REQ-CCR-003 Crash Reconciliation | Modelled | Expired-claim takeover emits reconciliation rather than blind replay |
| REQ-CCR-004 Bounded Retry | Modelled | Pure four-attempt policy and generated operation schedules |
| REQ-CCR-005 Repository Serialization | Not implemented | Real Git reservation/locking tests required |
| REQ-CCR-006 Runtime Bootstrap | Not implemented | Temporary persisted Idle path remains |
| REQ-CCR-007 Cancellation | Modelled | Claim revocation and visible cancelled-state test |
| REQ-CCR-008 Deletion | Modelled | Hidden deletion-pending tombstone test |
| REQ-CCR-009 Durable Scheduling | Not implemented | Deadline-aware production scheduler required |
| REQ-CCR-010 Deterministic Verification | Partial | 512 generated protocol schedules plus checked-in minimized regressions |

## Merge Gate

The async creation feature is not ready to merge until the production database, worker, Git/resource adapters, runtime bootstrap, and recovery UI conform to the normative protocol and pass deterministic, adapter, and end-to-end verification.
