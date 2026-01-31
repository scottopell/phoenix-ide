---
created: 2025-01-29
priority: p2
status: ready
---

# Implement sub-agent spawning (REQ-BED-008, REQ-BED-009)

## Summary

Sub-agents enable parallel task execution by spawning independent child conversations.

## Design

See `specs/subagents/design.md` for full design document.

### Key Decisions

| Decision | Choice |
|----------|--------|
| Enter AwaitingSubAgents | Dedicated `SpawnAgentsComplete` event |
| Track during execution | `ToolExecuting.pending_sub_agents` accumulates |
| Sub→Parent notification | State machine defines event; executor routes |
| Cancel propagation | `CancellingSubAgents` state, wait for confirmation |
| Timeout | Executor concern → manifests as `ErrorKind::TimedOut` |
| Tool availability | Filter at LLM request time |
| spawn_agents input | Batch `{ tasks: [{ task, cwd? }, ...] }` |
| Initial message | Synthetic UserMessage with task text |
| DB filtering | `user_initiated = false` excludes sub-agents |

### State Machine Changes

- Add `pending_sub_agents` to `ToolExecuting`
- Add `CancellingSubAgents` state
- Add `Completed` / `Failed` terminal states
- Add `SpawnAgentsComplete` event
- Add `SubAgentOutcome` enum (Success/Failure)

### Property Invariants

- Fan-in conservation: `|pending_ids| + |completed_results| == N`
- Monotonicity: pending decreases, completed increases
- Terminal states are terminal (no transitions out)
- Unknown/duplicate agent_id rejected
- No nested sub-agents (tool filtering)

## Acceptance Criteria

- [ ] State machine: new states, events, transitions
- [ ] Property tests for all invariants
- [ ] `spawn_agents` tool (parent only)
- [ ] `submit_result` / `submit_error` tools (sub-agent only)
- [ ] Tool filtering by ConvContext.is_sub_agent
- [ ] Effect handlers: SpawnSubAgent, CancelSubAgents, NotifyParent
- [ ] Timeout support
- [ ] Integration tests

## Implementation Order

1. State machine changes + property tests
2. Tools implementation + filtering
3. Runtime/executor support
4. Integration tests
