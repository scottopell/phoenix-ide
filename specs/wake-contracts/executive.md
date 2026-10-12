# Wake Retirement — Executive Summary

## Decision and Scope

The user explicitly approved removing the durable wake feature. ADR-092 records
that decision; ADR-006, ADR-011, and ADR-012 remain historical context, not current
execution authority. The wake registration/router/profile contract and its Allium
models are retired, without a replacement framework or promise of redesign.

## Current Reality

This is a candidate retirement implementation. Operational wake removal and the
one-time additive database migration are being implemented and verified; this
summary does not claim completed integration, passing final tests, or deployment.

Historical wake workflows, effect payloads and statuses, bindings, receipts,
messages, deliveries, acceptance records, and links must remain intact. Additive
feature-retirement metadata makes those workflows inert without falsifying their
historical execution outcomes. Historical wake pointers remain evidence only.

Already-admitted DirectTurns, including ones originating in wake delivery, remain
normal direct-turn work with their existing ownership, recovery, cancellation,
and settlement authority. Unaccepted wake delivery must not admit new work.
Ordinary Bash waits, tmux operations, sub-agent fan-in, and Close are not replaced.

## Requirement Status

| Requirement | Status | Verification needed |
| --- | --- | --- |
| REQ-WAKE-RETIRE-001 no wake execution authority | Candidate implementation / verification ongoing | No registration, scheduler or worker selection, startup/recovery/continuation dispatch, runtime admission, or wake lifecycle blocking. |
| REQ-WAKE-RETIRE-002 additive retirement migration | Candidate implementation / verification ongoing | Migration coverage for all wake statuses; exact preservation of historical payloads, statuses, bindings, receipts, messages, acceptance records, and links; supported runner re-open behavior. |
| REQ-WAKE-RETIRE-003 already-admitted DirectTurns | Candidate implementation / verification ongoing | Existing accepted-turn ownership and settlement survive retirement; historical unaccepted delivery remains inert. |
| REQ-WAKE-RETIRE-004 history without replacement feature | Candidate implementation / verification ongoing | Historical pointers retained; ordinary tools, fan-in, direct-turn settlement, and Close retain their owning contracts. |
| REQ-WAKE-RETIRE-005 forward-only compatibility | Specified | Apply the existing compatibility policy; no downgrade or wake-restoration guarantee. |

## Verification Boundary

The implementation owner owns migration and runtime regressions and final project
validation. Spec-only validation does not establish those behaviors. Old wake
feature tests and prior coverage are historical evidence, not acceptance of the
retirement contract.

## Authorities

- `requirements.md`: retirement and historical-data contract.
- `../durable-workflows/requirements.md`: retained foundation, direct-turn, and creation behavior.
- `../compatibility/requirements.md`: forward migration and paired rollback policy.
- `../adrs/092_retire-wake-execution-and-preserve-history.md`: explicit removal decision.
