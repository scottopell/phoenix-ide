# Wake Feature Retirement and Historical Data

## User Story

As a Phoenix operator, I need removal of the wake feature to stop autonomous wake
execution without deleting historical evidence or interrupting an already-admitted
direct turn.

## Scope

Wake contracts are retired. This specification governs feature retirement and
historical data only; it grants no wake registration, observation, delivery,
continuation transfer, or runtime-resume capability. Shared durable-workflow,
direct-turn, conversation-creation, and ordinary tool behavior remain governed by
their owning specifications.

## Requirements

### REQ-WAKE-RETIRE-001: No Wake Execution Authority

THE SYSTEM SHALL NOT admit new wake workflows or register wake contracts
AND SHALL NOT activate workers, observations, schedules, canonical delivery,
runtime admission, provider requests, or tool execution from wake state.

WHEN startup, recovery, continuation, or lifecycle processing encounters historical
wake workflows, bindings, receipts, deliveries, or pointers
THE SYSTEM SHALL treat them as historical data without execution authority
AND SHALL NOT reconcile, expire, cancel, transfer, deliver, or resume them as live
wake work.

Historical wake state SHALL NOT make a conversation runtime-busy or block lifecycle
admission, and SHALL NOT require wake settlement before ordinary Close can proceed.

### REQ-WAKE-RETIRE-002: Additive Retirement Migration

WHEN the wake-retirement migration is applied
THE SYSTEM SHALL atomically record feature-retirement metadata for every historical
wake workflow, independently of its execution status
AND SHALL preserve the existing workflow and effect payloads, statuses, generations,
attempts, schedules, bindings, terminal evidence, receipts, canonical deliveries,
messages, acceptance records, and message/delivery links.

The migration SHALL be one-time and additive. It SHALL NOT rewrite a historical
status to imply cancellation, completion, failure, or settlement, SHALL NOT fabricate
a receipt or transcript event, and SHALL NOT delete wake history. Applying the
recorded migration again through the supported migration runner SHALL add no
further semantic change.

Retirement metadata SHALL be the explicit feature-retirement fact; historical
execution status SHALL NOT serve as active-work authority.

### REQ-WAKE-RETIRE-003: Already-Admitted Direct Turns Remain Ordinary Work

WHEN a direct turn has already been durably admitted, including a turn whose
historical origin or transcript references wake delivery
THE SYSTEM SHALL retain its exact accepted-turn identity, generation, payload,
ownership, and canonical message/delivery links
AND SHALL process it through the normal direct-turn execution, recovery,
cancellation, and terminal-settlement authorities.

Wake retirement SHALL NOT revoke, re-admit, duplicate, or specially settle that
turn. An unaccepted historical wake delivery SHALL NOT be promoted into a new
direct turn.

### REQ-WAKE-RETIRE-004: History Is Not a Replacement Feature

THE SYSTEM SHALL preserve historical wake pointers and records as evidence,
not as a registration API, scheduler, router, polling service, or deferred execution
framework.

Wake retirement SHALL NOT replace direct-turn, child, tool, shared-`WorkScope`,
or lifecycle settlement machinery. Ordinary Bash `peek`, `wait`, and `kill`, tmux
operations, and sub-agent fan-in remain governed by their owning contracts.

### REQ-WAKE-RETIRE-005: Forward-Only Compatibility

THE SYSTEM SHALL apply wake retirement under the forward database migration policy
in `specs/compatibility/requirements.md` REQ-COMP-002
AND SHALL NOT promise that an older binary can open or execute against the migrated
database, or that retired wake execution can be restored by binary rollback.

## Reserved Historical Identifiers

REQ-WAKE-001 through REQ-WAKE-018 are retired identifiers, retained for historical
traceability only and SHALL NOT authorize feature behavior. The decisions behind
them remain recorded in ADR-006, ADR-011, and ADR-012; ADR-092 supersedes their wake
execution obligations without rewriting that history.

## Related Contracts

- `specs/durable-workflows/requirements.md`: shared foundation and direct-turn authority.
- `specs/bash/requirements.md`: ordinary process execution and synchronous waits.
- `specs/tmux-integration/requirements.md`: tmux resource identity and lifecycle.
- `specs/subagents/requirements.md`: normal parent fan-in and terminal evidence.
- `specs/work-lifecycle/requirements.md`: ordinary Close and resource retirement.
- `specs/compatibility/requirements.md`: forward migration and paired rollback policy.

No replacement wake design or future reintroduction is required by this contract.
