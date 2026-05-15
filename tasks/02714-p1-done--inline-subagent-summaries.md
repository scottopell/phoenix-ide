# Inline time-evolving sub-agent activity view

## Problem

The `spawn_agents` tool output currently gives users two extremes:

- parent view: a compact Sub-agents block with task/status rows and, after completion, a final submitted result summary
- child view: open the full sub-agent conversation in a separate route

That misses the useful middle ground: an inline, parent-local view of what each sub-agent is doing over time. The desired shape is closer to a compact pseudo-transcript:

```text
<sub-agent ABC purpose="Review telescope config">
  20+ tools
  <thinking>the user wants ...</thinking>
  <bash cmd="ls /root/var">...</bash>
  <read_file path="..."></read_file>
  ...
</sub-agent ABC>

<sub-agent EDF purpose="Propose alternative strategy">
  ...
</sub-agent EDF>
```

The goal is not merely to show a bigger final `submit_result`. The parent conversation should show sub-agent activity as it evolves: child agent text/thinking, tool calls, tool results, running/completed state, and final result/error — without forcing navigation away from the parent conversation.

## Current findings

- Backend already creates each sub-agent as a real child conversation; the `agent_id` used in UI is the child `conversation_id`.
- The parent conversation currently receives only coarse sub-agent state:
  - while running: `awaiting_sub_agents` with `pending` and `completed_results`
  - when all finish: `PersistSubAgentResults` updates the original `spawn_agents` tool-result message with `display_data.type = "subagent_summary"` and emits `message_updated`
- The parent stream does **not** currently include the child conversation’s incremental messages/tool events.
- The frontend already has full conversation APIs and SSE support for any conversation id:
  - `GET /api/conversations/:id`
  - `GET /api/conversations/:id/stream`
  - `GET /api/conversations/:id/slug`
- Therefore, the preferred design is likely: render an inline child-conversation activity panel by lazily fetching/subscribing to each child conversation, not by duplicating all child events into the parent stream. Add new parent-side SSE data only if the child-stream approach proves insufficient.

## Scope

Implement an inline, time-evolving activity view for sub-agents inside the parent conversation’s sub-agent/tool output area.

### Required behavior

1. Each sub-agent row/card shows:
   - status: running / success / failure / timeout
   - purpose/task text
   - tool/activity count summary where available or derivable
   - open-child-conversation affordance
   - an inline activity panel that can be expanded/collapsed

2. The inline activity panel shows a compact transcript of the child conversation:
   - agent text / reasoning-like narrative already present in messages
   - tool calls, including tool name and concise input summary (`bash ls /root/var`, `read_file path`, etc.)
   - tool results in collapsed/preview form using existing tool rendering patterns where possible
   - final `submit_result` / `submit_error` outcome clearly marked

3. The inline activity panel updates over time while the sub-agent is running:
   - initial open should fetch the child conversation’s current messages
   - while open, it should subscribe to the child conversation’s SSE stream (or another justified live source) and apply child `message`, `message_updated`, `token`, and `state_change` events locally
   - when collapsed, avoid unnecessary N-way streaming unless a deliberate lightweight summary update path is implemented

4. The persistent completed `spawn_agents` tool result still renders useful final summaries on reload:
   - existing `display_data.type = "subagent_summary"` remains supported
   - reconnect/reload should recover final results from parent persisted data
   - opening the inline activity panel after reload should fetch the child conversation history

5. Preserve navigation to the full child conversation. The inline view is a middle layer, not a replacement.

6. Outcome rendering must remain distinct for success, failure, and timeout. Before changing this, verify current behavior visually and in code; do not assume it is missing. If existing success/failure indicators already work, preserve them and only fill actual gaps (for example, stale TS typing around Rust `timed_out`).

## SSE / data contract approach

Do not start by adding a duplicate parent-side event stream. First implement against the existing child conversation APIs/SSE, because child conversations already have the complete timeline.

Only add backend/SSE data if there is a documented gap, such as:

- child conversation streams cannot be safely opened from the parent UI
- hidden sub-agent conversations are not accessible through existing APIs
- required event data is only available inside the child runtime and not persisted/streamed
- too many child streams create unacceptable load, requiring a parent-side summarized activity event

If new data is needed, it must avoid parallel representations of the same semantic value. Prefer a typed child-activity reference/projection over copying full child messages into parent `display_data`.

## Suggested implementation plan

1. Run the app and QA the existing behavior before coding:
   - create/use a conversation that spawns multiple sub-agents
   - verify current running, success, failure, and timeout rendering
   - capture what currently changes over time in the parent view
   - verify whether the child conversation route shows the desired timeline data

2. Audit data contracts:
   - Rust `SubAgentOutcome` includes `success`, `failure`, `timed_out`
   - frontend `SubAgentOutcome` in `ui/src/api.ts` currently appears to model only success/failure; align it only if confirmed stale
   - parent `display_data.subagent_summary` carries final results but not full child timeline

3. Build a reusable inline child conversation viewer component:
   - input: `agentId`, `task`, status/outcome
   - fetch initial child messages via `GET /api/conversations/:id`
   - when expanded and child is running, subscribe to `/api/conversations/:id/stream`
   - reuse existing message/tool rendering helpers where possible, but render in a compact nested style

4. Add compact transcript rendering:
   - group or summarize noisy runs of tool calls
   - show a clear tool-count/activity summary
   - keep outputs collapsed by default, with previews
   - show streaming child agent text when available

5. Integrate the component in both places sub-agent results appear:
   - live `SubAgentStatus` block for pending/completed sub-agents
   - persistent `SubAgentSummary` inside the completed `spawn_agents` tool result

6. Avoid excessive streams:
   - subscribe only for expanded/visible panels, or otherwise justify the resource tradeoff
   - clean up EventSource subscriptions on collapse/unmount

7. Tests:
   - unit/reducer coverage for child inline stream application if new local reducer code is added
   - component tests for compact transcript rows from representative child messages: agent text, tool_use, tool result, final submit result/error
   - existing `message_updated`/`subagent_summary` tests remain passing
   - typecheck catches Rust/TS outcome drift

8. Run verification:
   - `./dev.py up` / restart as needed
   - browser QA with an actual sub-agent run
   - verify the inline panel changes over time while a child is running
   - verify reload recovers completed parent summaries and can fetch child history
   - run `./dev.py check`

## Acceptance criteria

- Parent conversation provides an expandable inline activity view for each sub-agent.
- The inline view shows the sub-agent timeline, not only its final submitted result.
- While a sub-agent is running, an expanded inline view updates as the child agent emits text/tool activity.
- The inline view includes compact representations of agent text, tool calls, tool results, and final outcome.
- The default collapsed view stays compact and shows status, purpose, and activity count/summary.
- Success, failure, and timeout states render distinctly, preserving any status UI that already works today.
- Opening the full child conversation remains available.
- Reload/reconnect preserves final parent summaries and can lazy-load child history for the inline view.
- No duplicate parent-side SSE/message representation is introduced unless the implementation documents why existing child conversation APIs/SSE are insufficient.
