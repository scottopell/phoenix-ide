# Restore generic Work sub-agent options alongside named personas

## Problem

Recent named-agent support made `spawn_agents.tasks[].agent_type` an enum of discovered named personas. In this worktree, that enum exposes only:

- `timer-smell-hunter`
- `user-journey-planner`

The schema still allows `mode: "work"`, but there is no general-purpose coding/review/research persona available through `agent_type`. That makes Work conversations unable to delegate arbitrary implementation work to sub-agents, even though Work sub-agents still have the full writing registry (`bash` + `patch`) and the sub-agent specs allow unnamed tasks.

## Evidence

- `crates/phoenix-tools/src/subagent.rs` renders `agent_type` only from the discovered named-agent catalog.
- `crates/phoenix-tools/src/lib.rs::ToolRegistry::for_subagent_work()` still supports real Work sub-agents with `bash` and unrestricted `patch`.
- `crates/phoenix-ide/src/runtime/executor.rs::handle_spawn_agents_tool()` allows tasks with `agent_type: None` and resolves their mode from task field/defaults.
- The API tool schema shown to the agent currently advertises only named `agent_type` values, so skills expecting generic implementation/review/research sub-agents have no obvious valid persona to choose.

## Goal

Make generic sub-agent delegation usable again while preserving named-agent support.

## Candidate fix

1. Keep `agent_type` optional.
2. Make the `spawn_agents` description/schema explicitly document that omitting `agent_type` spawns a generic Phoenix sub-agent.
3. Consider adding built-in generic named personas (for example `coding-agent`, `review-agent`, `research-agent`) if the intended UX is to require `agent_type` for all non-trivial delegation.
4. Add tests that the rendered schema does not imply discovered named agents are the only possible sub-agent kinds, and that Work-mode generic tasks remain supported.
5. Audit skills such as `serial-sub-agent` for assumptions about generic sub-agent availability.

## Acceptance

- A Work conversation can spawn an unnamed generic Work sub-agent for arbitrary coding/test implementation work.
- Named personas remain discoverable and validated.
- Unknown `agent_type` is still rejected.
- The tool schema no longer makes the two discovered named personas look like the only sub-agent surface.
