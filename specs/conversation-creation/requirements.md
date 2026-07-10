# Durable Conversation Creation

## User Story

As a developer starting a Phoenix conversation, I need the conversation shell to appear immediately and provisioning to complete exactly once despite retries, process failure, or concurrent workers, so that creation never duplicates Git resources, corrupts conversation state, or leaves me without a recovery action.

## Requirements

### REQ-CCR-001: Durable Acceptance

WHEN a structurally valid creation request is accepted
THE SYSTEM SHALL atomically persist a navigable conversation shell and its complete creation intent before acknowledging acceptance
AND filesystem, Git, reference expansion, attachment finalization, and runtime bootstrap SHALL occur after acceptance

### REQ-CCR-002: Exclusive Provisioning Authority

WHEN a worker begins or resumes provisioning
THE SYSTEM SHALL grant that worker one time-bounded creation claim with a monotonically increasing generation
AND every authoritative provisioning update SHALL require the current claim and generation

WHEN a worker reports a result after losing its claim
THE SYSTEM SHALL reject the result without changing conversation, job, message, or resource ownership state

### REQ-CCR-003: Crash Reconciliation

WHEN a creation claim expires during an external operation
THE SYSTEM SHALL fence that generation from further authoritative updates
AND a replacement worker SHALL inspect durable reservations and observed external state before resuming, adopting, conflicting, or cleaning the operation

THE SYSTEM SHALL NOT infer that an external operation failed merely because its acknowledgement was not persisted

### REQ-CCR-004: Bounded Retry

WHEN provisioning fails for a transient reason
THE SYSTEM SHALL durably schedule no more than four total attempts using delays of 2 seconds, 10 seconds, and 30 seconds between subsequent attempts

WHEN the retry budget is exhausted
THE SYSTEM SHALL preserve a failed conversation record with the original creation intent and final error

WHEN provisioning fails for a permanent reason
THE SYSTEM SHALL preserve the failed conversation without an automatic retry

### REQ-CCR-005: Repository Mutation Serialization

WHEN creation mutates Git refs or worktrees
THE SYSTEM SHALL serialize mutation by canonical repository identity across live Phoenix processes
AND cleanup SHALL remove only resources whose durable ownership still belongs to the cleanup operation

### REQ-CCR-006: First-Class Runtime Bootstrap

WHEN provisioning submits an initial message
THE SYSTEM SHALL transition directly from provisioning into the normal conversation lifecycle through an idempotent bootstrap operation
AND the system SHALL NOT temporarily persist an idle conversation solely to initialize a runtime

### REQ-CCR-007: Cancellation

WHEN a user cancels an accepted, claimed, or retry-scheduled creation
THE SYSTEM SHALL immediately revoke the active generation
AND SHALL preserve a visible cancelled conversation with its original creation intent
AND SHALL reconcile owned resources asynchronously

WHEN reconciliation completes
THE SYSTEM SHALL offer Start over and Delete for the cancelled record

### REQ-CCR-008: Deletion During Creation

WHEN a user deletes a non-ready creation record
THE SYSTEM SHALL immediately omit it from normal user-facing conversation surfaces
AND SHALL retain an internal deletion-pending record until owned resources are safely reconciled
AND SHALL physically delete the record only after reconciliation succeeds

### REQ-CCR-009: Durable Scheduling

WHEN an accepted job, retry deadline, or expired lease becomes eligible
THE SYSTEM SHALL make it discoverable without requiring another conversation request or process restart

### REQ-CCR-010: Deterministic Verification

WHEN the creation protocol is verified
THE SYSTEM SHALL exercise generated operation schedules containing concurrent claims, lease expiry, late results, crashes, retries, cancellation, deletion, and ambiguous external-effect completion
AND SHALL check lifecycle and ownership invariants after every generated operation
AND SHALL retain minimized failing schedules as deterministic regressions
