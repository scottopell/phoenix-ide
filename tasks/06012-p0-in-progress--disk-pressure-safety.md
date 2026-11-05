# Make Phoenix safe under extreme disk pressure

## Authorized scope
User approved implementation of four slices, in this order:
1. P0: safe ordinary tool-completion failure handling.
2. P0: safe startup and recovery for interrupted tools, active turns, and queued messages.
4. P1: bounded persistence and diagnostic-sink failure observability independent of successful disk writes.
3-obs. P1: disk-space observation and warning only.

Disk observations do not block admission, select a draining mode, automatically reopen admission, or claim readiness. No automatic cleanup, reserve files, storage isolation, generic retries, database rollback, or new recovery service is included.

## Problem and owning invariant
A production incident left local tool invocations apparently running for hours during host ENOSPC. The same reference lookup subsequently returned quickly. Missing logs/traces prevent attributing the exact original failed statement.

The verified implementation gap is that ordinary outcome processing can consume the sole outcome, fail a persistence/transition effect, log the error, and continue without an owned producer or settlement path. Every consumed outcome must select an established durable disposition or the existing bounded process-wide fail-stop behavior.

Use REQ-DWF-043's command-scoped durable-fact classification and REQ-BED-033's existing fail-stop contract. Preserve exact tool/assistant identities, generation fences, committed history, active direct-turn authority, accepted inputs, and queued steering. Do not replay uncertain external effects or represent local-authority uncertainty as semantic conversation state.

## Implementation and acceptance
- Reproduce ordinary completion failures at steering-drain preparation, checkpoint begin/write/commit, and subsequent state/replay writes. Test the real caller's disposition.
- Make missing authoritative results structurally select the existing failure contract. Retain outcomes only in an owned bounded settlement path; do not introduce an unbounded retry/spool subsystem.
- Audit startup/runtime reconstruction of required durable facts versus optional derived capabilities. Verify interrupted tools, active-turn ownership, queued messages, cancellation, reconnect, and process replacement without duplicate external execution or opening each transcript manually.
- Add bounded process-local persistence-stage/failure and logging/export failure observations. Diagnostics must not own runtime correctness or delay fatal cutoff. Distinguish execution/settlement observations from authoritative domain state.
- Report available bytes, sample age, and measurement errors for relevant filesystems using cheap sampling. Deduplicate identical filesystems and label server-local paths. Provide an observation-only low-space warning.
- Update relevant normative specs, executive status, and ADRs only as warranted; no expanded compatibility/recovery promises.
- Run deterministic fault tests, owning checks, codegen, the repository check, and user-visible validation. Use isolated test storage; never fill the production filesystem.
- Adversarially review immutable base/head; fix actionable findings. Open a ready PR and iterate until review is clean and all required CI checks are green. Do not merge/deploy as part of this task.

## One-time development preparation
The user separately authorized build-artifact cleanup freely to obtain safe development headroom. Keep source, production state, recovery evidence, credentials, and live non-build resources intact. Cleanup receipts stay in ignored local incident evidence; this is not a product cleanup feature.

## Investigation context
Detailed production evidence and the initial triage are retained under ignored .phoenix/incident-20261105-ember/ (codex on ESS). These machine-local pointers are not GitHub-verifiable delivery evidence. The exact original incident error remains unproven; code-level regressions must establish the failure mechanism fixed by this PR.
