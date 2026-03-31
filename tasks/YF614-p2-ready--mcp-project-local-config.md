---
created: 2026-03-30
priority: p2
status: ready
artifact: pending
---

# MCP: Discover project-local .mcp.json from conversation working directory

## Summary

`McpClientManager::read_all_configs()` scans `.mcp.json` using
`std::env::current_dir()` -- the Phoenix process CWD, not the conversation's
project directory. This means project-local MCP configs are never discovered
unless Phoenix happens to be started from that project root.

## Expected Behavior

Each conversation has a `working_dir` (the project root). When a conversation
starts, its `.mcp.json` should be included in MCP tool discovery. This means
MCP discovery needs to become project-aware:

- Global servers (from `~/.claude.json`, etc.) are shared across all conversations
- Project-local servers (from `{project_root}/.mcp.json`) are scoped to
  conversations in that project

## Design Considerations

- `read_all_configs` is currently called once at startup (background discovery)
  and on reload. It would need to accept a project root parameter.
- Project-local servers should probably be connected lazily (on first
  conversation in that project) rather than eagerly at startup.
- Server naming: a project-local server named "db" shouldn't collide with a
  global server named "db". Possible: prefix project-local names with project
  context, or keep separate registries.
- Ties into YF613 (dynamic tool resolution) -- project-local tools need to
  appear in the right conversations.

## Context

Discovered during MCP Phase 4 implementation. The `.mcp.json` CWD path at
`src/tools/mcp.rs` line ~626 silently resolves to the wrong directory.
