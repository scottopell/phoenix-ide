---
created: 2026-03-13
priority: p3
status: ready
artifact: pending
---

# Collapse or hide think tool results in message display

## Problem

The think tool's internal reasoning is displayed prominently in the message
list -- both the `<thinking>` content block (the agent's raw thoughts) and
the tool result ("Thoughts recorded. Now continue with your response to the
user."). This clutters the conversation with internal reasoning that the user
didn't ask to see.

The think tool description says "not shown to the user" but the UI shows
everything.

## What to Do

In the agent message rendering (`AgentMessage` component or tool result
rendering), detect think tool calls and either:

1. **Collapse by default**: Show a muted, collapsed "Thinking..." indicator
   that expands on click to reveal the thoughts. Similar to how tool outputs
   can be collapsed.

2. **Hide entirely**: Don't render think tool_use or its tool_result at all.
   The thoughts served their purpose (helping the LLM reason) and add no
   value to the user.

Recommendation: Option 1 (collapse). Some users may want to see what the
agent was thinking. A collapsed indicator with expand-on-click respects both
preferences.

The tool result text ("Thoughts recorded...") should never be shown -- it's
an implementation detail.

## Acceptance Criteria

- [ ] Think tool calls are collapsed/muted by default
- [ ] Think tool results ("Thoughts recorded...") are hidden
- [ ] User can expand to see the thinking content if desired
- [ ] Other tool calls are unaffected
