---
created: 2026-04-20
priority: p2
status: ready
artifact: src/state_machine/event.rs
---

# Review SpawnAgentsComplete and SubAgentResult placement in CoreEvent

## Summary

C1 placed SpawnAgentsComplete and SubAgentResult in CoreEvent because parents
receive sub-agent results when in AwaitingSubAgents. This is functionally
correct but these events are only meaningful for specific state variants.
Review whether they should stay in CoreEvent or move to a shared-but-scoped
location.

## Done When

Decision documented. If moved, all tests pass.
