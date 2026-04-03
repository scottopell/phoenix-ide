---
created: 2026-03-30
priority: p3
status: ready
artifact: pending
---

# MCP: Project/conversation-scoped enable/disable preferences

## Summary

The current MCP server disable feature (Option A) uses a flat
`mcp_disabled_servers` table keyed only by server name. This is global --
disabling "playwright" disables it for every conversation.

Once project-local MCP servers exist (YF614), users will need scoped
preferences:

- Disable a global server for a specific project (e.g., "don't use the
  Playwright MCP in my backend-only repo")
- Enable a project-local server only for conversations in that project
  (this happens naturally via YF614 discovery)
- Per-conversation overrides (unlikely to be needed, but structurally possible)

## Design Sketch

Extend `mcp_disabled_servers` with an optional `project_id` column:

```sql
CREATE TABLE mcp_disabled_servers (
  server_name TEXT NOT NULL,
  project_id  TEXT,  -- NULL = global preference
  UNIQUE(server_name, project_id)
);
```

Resolution: a server is disabled for a conversation if it's disabled globally
(project_id IS NULL) OR disabled for that conversation's project. Per-project
enable could override global disable if needed (add an `enabled` boolean).

## Dependencies

- YF614 (project-local .mcp.json discovery)
- YF613 (dynamic tool resolution -- disabled state needs to be checked at
  tool-resolution time, not just at startup)

## Context

Deferred during MCP Phase 4 to keep the initial disable feature simple.
The flat global model covers the immediate need (disabling redundant servers
like Playwright when Phoenix has built-in browser tools).
