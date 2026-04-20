---
created: 2026-04-20
priority: p3
status: ready
artifact: src/state_machine/transition.rs
---

# Full ConvMode/ConvState co-constraint enforcement

## Summary

Finding #8 added a mode guard on propose_task, but other invalid (mode, state)
combinations are still representable: Direct + AwaitingUserResponse,
Direct + AwaitingTaskApproval (if tool registry drifts), etc. The tool registry
is the primary guard; the state machine is defense-in-depth.

## Done When

Property test covers all invalid (mode, state) combinations.
Decision on whether to add structural enforcement (mode-specific state enums)
or keep as runtime guards + tests.
