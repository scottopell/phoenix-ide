---
created: 2026-01-31
priority: p2
status: done
---

# Emit Granular State Change Events for UI

## Summary

Update the state machine transitions to emit `state_change` SSE events on every state transition, with structured `state_data` payloads that provide enough detail for the UI to render progress indicators.

## Context

The mobile web UI (REQ-API-011) displays a state machine visualization as its primary feedback mechanism. Currently, `state_change` events are only emitted for some transitions (error states, retries). The UI needs events for:

- Entering `tool_executing` state (to show which tool and queue depth)
- Entering `llm_requesting` state (to show thinking indicator)
- Entering `awaiting_sub_agents` state (to show sub-agent count)

Without these events, the breadcrumb trail in the UI doesn't update during tool execution, making it appear stuck.

## Acceptance Criteria

- [ ] `state_change` event emitted when entering `ToolExecuting` state
- [ ] `state_change` event emitted when entering `LlmRequesting` state  
- [ ] `state_change` event emitted when entering `AwaitingSubAgents` state
- [ ] `state_data` includes structured info per REQ-API-005a:
  - `tool_executing`: `{current_tool: {name, id}, remaining_count, completed_count}`
  - `llm_requesting`: `{attempt}`
  - `awaiting_sub_agents`: `{pending_count, completed_count}`
- [ ] Web UI breadcrumb trail updates in real-time during tool execution

## Notes

- Look at `src/state_machine/transition.rs` for where to add `Effect::notify_state_change`
- The `Effect::notify_state_change` helper already exists in `effect.rs`
- Test with the web UI at `/static/index.html` to verify breadcrumbs update
- See `static/app.js` `updateBreadcrumbsFromState()` for how UI consumes these events
