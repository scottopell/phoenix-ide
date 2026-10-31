# ADR-192: Goal admission requires explicit automatic continuation

- **Status:** Accepted
- **Date:** 2026-10-29
- **Affects:** REQ-GOAL-AC-001–004

## Context

The user commissioned an agent-accessible automatic-continuation setting and made enabled automatic continuation a prerequisite for Goal admission. Existing Goals and history, Complete/Suspend/Stop, and explicit lifecycle authority must remain intact. This work spans the Goals writer and a separately owned native settings adapter; it does not authorize merging or deploying either PR.

## Options considered

1. Check only HTTP and native adapters: duplicates policy and leaves generated-host writes able to bypass admission.
2. Keep the setting permanently enabled while a Goal is active: adds an uncommissioned restriction on the user's OFF gesture and legacy Goal management.
3. Check new/replacement admission in the shared Goal transaction and leave existing evidence and safety controls independent: selected.

## Decision

`apply_goal_tx` requires the same owner's persisted automatic-continuation preference for `GoalChange::Create`, including replacement. It preserves retained-action replay before checking the new-admission prerequisite. SQLite transaction serialization orders preference updates and Goal admission; no second cached authority is introduced. Disabled admission returns `goal_requires_auto_continue` through the shared HTTP/native error mapping and does not enable the setting.

Inactive Goal identities remain inactive. A new Goal identity is required to replace a Suspended, Complete or Exhausted Goal; an existing active Goal's Underway/Blocked metadata updates are not new admission. Reads, history, progress, safety exits and Stop remain available when the setting is disabled. No persistent active-Goal/setting lock is added.

The Goals owner owns shared admission consistency and regression proof. The native Tools owner owns `set_auto_continue` and the lifecycle/settings adapters, using canonical ProductConversation authority and the shared setter. Coordinator-led assignment and follow-through do not require an extra deployment-integrator permission hop. Neither this decision nor enabling the preference creates a scheduler, wakes exhausted work or grants merge/deploy approval.

## Consequences

Existing Goal rows/history need no migration or reactivation. Focused tests cover disabled create/replacement on owner HTTP and registered native tools, exact accepted replay while disabled, preserved legacy management and the native runtime execution boundary. The native setting tool is qualified by its owner separately; the Goals PR does not claim it is already implemented.
