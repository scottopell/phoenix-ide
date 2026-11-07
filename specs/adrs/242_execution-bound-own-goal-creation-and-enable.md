# ADR-242: Execution-bound own Goal creation and explicit continuation enable

Status: Accepted
Date: 2026-11-07
Affects: REQ-SGT-001–008, REQ-GOAL-AC-001–004

## Context

Ordinary agents needed to save their own commissioned objective. Existing user and Coordinator Goal creation required a settled target, so self-calling that path during a turn could not work. The user approved create-only authorship, then explicitly added a separate own-conversation enable-auto-continuation operation. Neither commission included editing, replacement, reactivation, dispatch, or remote targeting.

The preference was a Boolean. A Boolean alone could not distinguish an unchanged disabled preference from a human disable after agent admission, including repeated false writes and disable/enable ABA. A direct settings write followed by a separate tool-result write also could not make response-loss replay safe.

## Decision

Use two strict ordinary-agent tools with server-derived identity: `enable_auto_continuation {}` and `create_goal { objective }`. Preserve the existing enabled-preference prerequisite for Goal creation and the settled-target rules for external writers.

Represent effects as typed pending tool-round results and finalize them in the existing canonical SQLite transaction with the selected state, retained result and carried replay/proposal/terminal obligation. Reuse the shared Goal writer and preference persistence helper; do not introduce a scheduler or independent receipt framework.

Add a durable per-owner disable epoch and a normalized per-turn admission row. Every explicit disable advances the epoch, including false-to-false writes. A self-operation compares its immutable admitted epoch and execution/control generation inside the mutation transaction. Existing canonical receipts bind the pending authority and selected result evidence; retained replay returns history without repeating the write.

## Alternatives considered

- Calling the settled-target HTTP path would reject the active caller or require weakening unrelated admission.
- Mutating preferences in tool execution before checkpointing could lose the paired result and overwrite a later human disable on retry.
- Reading a fresh epoch at tool execution would miss a human disable after turn admission.
- Reusing Stop's control version for preference changes would conflate independent controls and expand Stop semantics.
- A combined enable-and-create operation would hide the automation change inside objective creation.

## Consequences

A human disable committed after admission fences that turn, even after another enable. A later genuinely new turn may receive fresh authority; replay cannot refresh an old turn. Enable creates no Goal, and create changes no preference or current-turn Goal binding. Every existing Goal disposition blocks create. Neither tool grants scope, budget, execution, Stop-release, or Coordinator authority.

The migration is forward-only and the new admission evidence applies to newly admitted turns. Old in-flight executions lacking that evidence refuse self-management rather than acquiring fresh authority. This does not promise mixed-version live replacement or expand compatibility guarantees.
