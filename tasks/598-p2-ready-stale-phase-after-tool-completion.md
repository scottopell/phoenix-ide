---
created: 2025-07-13
priority: p2
status: ready
---

# UI messages missing after server restart, phase indicators disconnected from visible content

## Summary

After a server restart mid-conversation (e.g. triggered by a `./dev.py prod deploy` that restarts the systemd service), the UI shows phase indicators (state bar, breadcrumb bar, Cancel button) that reflect the server's current state, but the corresponding message content is not visible. The user sees old completed messages but current status indicators, with no way to understand what's actually happening.

## Reproduction (observed in prod)

1. Agent runs bash: `cargo fmt && git add ... && git commit -m "fmt"` → completes with ✓
2. Agent decides to run `./dev.py prod deploy` → this restarts the phoenix-ide systemd service
3. New server process detects interrupted conversation, auto-continues
4. LLM responds (102 tokens, ~5s), server starts executing new bash tool (`./dev.py prod deploy`)
5. **User sees:** the old git commit with ✓, Cancel button, breadcrumb `bash`, state bar `🟡 bash`
6. **User does NOT see:** the LLM's response text, the new bash tool_use block for the deploy command

## Root Cause

The server restarted, breaking the SSE connection. On reconnection, the client either:
- Did not replay messages that were created after the restart (sequence ID gap?)
- Or the new messages (LLM response + tool_use) were created server-side but the SSE replay didn't include them

Meanwhile, the `state_change` SSE events ARE being received (phase=tool_executing, tool=bash), so the status indicators update but the messages they refer to are invisible.

## What the User Should See

1. ✅ Completed git commit bash with ✓ (visible — correct)
2. ❌ LLM text response deciding to deploy (MISSING)
3. ❌ New bash block showing `./dev.py prod deploy` with in-progress spinner (MISSING)
4. ✅ State bar `🟡 bash` (visible — correct for #3, but #3 is invisible)

The phase indicators are technically correct for the server's state — the bug is that the messages aren't visible, so the indicators appear stale/wrong.

## Possible Causes to Investigate

1. **SSE reconnection sequence ID tracking:** Does the client correctly request replay from the last received sequence ID after a connection drop? Or does it re-request from too early/too late?
2. **Message persistence timing:** Are the new messages (LLM response, tool_use) persisted to SQLite before the state_change event is sent? If state_change arrives first and the client re-fetches, the messages might not exist yet.
3. **Auto-continue message visibility:** When the server auto-continues an interrupted conversation, are the new messages being broadcast on the SSE channel for that conversation?

## Acceptance Criteria

- [ ] After server restart + auto-continue, SSE-reconnected clients see all new messages
- [ ] Phase indicators always correspond to visible message content
- [ ] If messages can't be delivered, phase indicators degrade gracefully (e.g. show "reconnecting..." not a stale tool name)
- [ ] Cancel button is only shown when the user can see what they'd be cancelling
