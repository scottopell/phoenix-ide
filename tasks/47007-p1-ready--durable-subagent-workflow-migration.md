# Migrate sub-agents to first-class durable workflows

## Goal

Replace the old-system/new-system boundary around sub-agents with a first-class durable workflow profile. The child conversation or agent ID is the stable resource identity; parent scope is routing/ownership metadata, not identity. Preserve exact terminal causes, recover from persisted state, and transfer delivery ownership without changing resource identity.

## Scope

- Specify the sub-agent workflow profile and lifecycle precisely.
- Inventory direct orchestration, retry, timeout, cancellation, continuation, and terminal-delivery paths.
- Add shadow parity and deterministic schedule coverage before authority cutover.
- Migrate authority incrementally behind selectors; retain rollback and drain proof.
- Retire direct orchestration only after zero blocking divergence and complete drain proof.

## Acceptance criteria

- [ ] Stable child identity is structural and independent of parent WorkScope.
- [ ] Exact terminal-cause taxonomy survives restart and replay.
- [ ] Claims, attempts, deadlines, cancellation, continuation, and runtime acceptance use durable workflow authority.
- [ ] Legacy and engine projections pass deterministic and representative production parity campaigns.
- [ ] Cutover, rollback, and retirement follow the durable-workflow migration register.
