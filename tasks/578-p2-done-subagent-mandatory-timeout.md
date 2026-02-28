---
created: 2026-02-28
number: 578
priority: p2
status: done
slug: subagent-mandatory-timeout
title: "Sub-agent mandatory timeout with deadline enforcement"
---

# Sub-Agent Mandatory Timeout

## Context

Read first:
- `specs/subagents/requirements.md` — REQ-SA-006
- `specs/subagents/design.md` — "Timeout Behavior" section
- `specs/bedrock/requirements.md` — REQ-BED-026
- `specs/bedrock/design.md` — Appendix A (FM-6)

FM-6: Sub-agents have no timeout. `SpawnAgentSpec.timeout` is always `None`. A parent
can wait in `AwaitingSubAgents` forever.

## What to Do

1. **Change `timeout` from `Option<Duration>` to `Duration`** on the sub-agent spawn
   config. The caller must make a conscious decision. Pick a sensible default for the
   `spawn_agents` tool (5 minutes suggested — long enough for real work, short enough
   to catch stuck agents).

2. **Add `deadline: Instant` to `AwaitingSubAgentsState`** (or equivalent). Set it when
   transitioning into the state: `Instant::now() + max_timeout` where `max_timeout` is
   the longest timeout among pending sub-agents.

3. **In the executor**, add a timeout arm to the select loop for `AwaitingSubAgents`:

   ```rust
   select! {
       result = sub_agent_rx.recv() => { /* process normally */ }
       _ = tokio::time::sleep_until(deadline) => {
           // Timeout: send UserCancel to all pending sub-agents
           // They will respond with SubAgentOutcome::TimedOut
       }
   }
   ```

4. **Add `SubAgentOutcome::TimedOut` handling** in `handle_outcome` — treat the same as
   `Failure` but with `ErrorKind::TimedOut`.

5. **Update the bounded buffer** for early results: use `mpsc::channel(pending_count)`
   instead of unbounded `Vec`. Capacity = exact number of sub-agents.

## Acceptance Criteria

- `timeout` is `Duration`, not `Option<Duration>`, on spawn config
- `spawn_agents` tool provides a default timeout
- Stuck sub-agents are terminated after timeout
- Timeout produces `SubAgentOutcome::TimedOut` to parent
- Bounded buffer for early results (capacity = sub-agent count)
- `./dev.py check` passes

## Dependencies

- Task 577 (typed effect channels — provides SubAgentOutcome type)

## Files Likely Involved

- `src/state_machine/state.rs` — AwaitingSubAgentsState, deadline field
- `src/state_machine/transition.rs` — timeout handling
- `src/tools/spawn_agents.rs` — default timeout value
- `src/runtime/executor.rs` — select loop timeout arm
- `src/runtime/` — sub-agent spawning, bounded channel
