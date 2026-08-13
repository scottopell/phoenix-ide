# Coordinator-directed lifecycle implementation worker

Enter Work mode so this conversation can execute only the lifecycle work explicitly assigned by the coordinator through subsequent steering messages.

## Scope gate

No implementation workstream is selected by this task. Before editing, the worker must receive a concrete coordinator assignment and restate:

- `Requires`
- `Provides`
- `Forbids`
- authority before and after
- merged-main invariant
- expected durable output
- validation evidence
- feedback routing

The worker must stop if steering conflicts with normative requirements, Allium specifications, accepted ADRs, or the assigned taskmd authority.

## Constraints

- Follow the approved lifecycle sequence and do not absorb downstream milestones.
- Do not reopen the settled product decisions supplied by the coordinator.
- Read all applicable requirements, Allium, executive documents, ADRs, and assigned task authority before changing specified behavior.
- Use the `phoenix-development` skill for implementation.
- Classify discoveries as `FIX-NOW`, `PREREQUISITE`, `DEFER:<task or milestone>`, or `UNRELATED:<workstream>`.
- Return prerequisite defects to their owning boundary; do not introduce local authority workarounds.
- Do not edit Issue #651’s generated body or another owner’s roadmap workstream.
- Do not merge pull requests.

## Completion

Completion criteria, validation, commits, pushes, PR handling, and roadmap reporting will come from the coordinator’s concrete assignment and the higher-order authorities it identifies. Until that assignment arrives, make no repository or remote changes beyond this approved mode transition.
