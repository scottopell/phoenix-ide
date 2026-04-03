---
created: 2026-03-30
priority: p3
status: done
artifact: pending
---

# Dynamic MCP Tool Resolution

## Summary

MCP tools are snapshot-captured into each conversation's `ToolRegistry` at
runtime creation time. This means:

- Tools from servers that finish connecting after the runtime is created are
  invisible to that conversation.
- `POST /api/mcp/reload` adding new servers only affects conversations started
  after the reload.
- Removed servers leave stale tool entries in the LLM's tool list (calls fail
  with "server not connected").

Tool *execution* is already live (routes through `Arc<McpClientManager>`), so
crash/respawn is transparent. The frozen part is tool *discovery* -- which
names appear in the LLM's tool definitions list.

## Proposed Fix

Resolve MCP tools dynamically from the manager at LLM-request time instead of
snapshotting into `ToolRegistry` at conversation start. Options:

1. **Lazy definitions**: `ToolRegistry::definitions()` queries `McpClientManager`
   on each call instead of iterating a static `Vec<Arc<dyn Tool>>`.
2. **Registry refresh**: Add a `refresh_mcp_tools()` method called before each
   LLM request round, diffing against the manager's current state.
3. **Hybrid**: Keep built-in tools static, resolve MCP tools dynamically via a
   trait object that delegates to the manager.

Option 3 is cleanest -- it preserves the existing `Tool` trait for built-ins
while making MCP resolution live.

## Practical Impact

Low urgency. Background discovery completes in seconds, so the snapshot is
nearly always complete by the time a user sends their first message. The reload
case is the main gap, and it's rare. Tracked for correctness, not user pain.

## Context

Discovered during code review of `feat/mcp-client` branch (Phase 1 + Phase 4).
See `src/runtime.rs` line 512 (`register_mcp_tools`) and
`src/tools/mcp.rs` `tool_definitions()`.
