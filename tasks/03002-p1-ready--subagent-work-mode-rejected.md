---
created: 2025-05-02
priority: p1
status: ready
artifact: src/tools.rs
---

## Work-mode sub-agents rejected by `spawn_agents` tool even when parent is in Work mode

When an agent invokes `spawn_agents` with `mode: "work"` from within a Work-mode conversation, the tool rejects the request with:

> Work sub-agents require the parent to be in Work mode. Use mode: 'explore' or omit mode for read-only sub-agents.

This is wrong — the parent IS in Work mode. The check is either not being evaluated correctly or is hardcoded to always deny `mode: "work"` sub-agents.

### Reproduction

From a Work-mode conversation, invoke `spawn_agents` with a task containing `mode: "work"`. The tool returns the error above instead of spawning the sub-agent.

### Expected behavior

Work-mode parents should be able to spawn Work-mode sub-agents. This is the intended design per REQ-PROJ-008 and `ToolRegistry::for_subagent_work()`.

### Relevant code

- `src/tools.rs` — `spawn_agents` tool implementation; look for where the Work-mode parent check lives
- `src/runtime/executor.rs` — sub-agent spawn logic (`SubAgentMode::Work` handling)
- `src/state_machine/state.rs` — `SubAgentMode` enum

### Priority note

This blocks the serial-sub-agent orchestration pattern entirely for Work tasks, forcing the orchestrating agent to implement everything inline (burning parent context). p1 because it defeats a key productivity pattern.
