---
created: 2026-04-07
priority: p3
status: ready
artifact: src/tools/mcp.rs
---

# Native MCP OAuth instead of shelling out to npx

## Problem

MCP server connections that require OAuth (e.g., Atlassian, Slack) shell out
to `npx` for the OAuth dance, which opens a browser tab on every reload/reconnect.
This is disruptive -- the user hits MCP reload and suddenly has browser tabs
opening for each OAuth-requiring server.

## What to do

Bring the OAuth token acquisition native to Phoenix:
- Cache OAuth tokens in the Phoenix DB or a dedicated credentials store
- Only open a browser for initial authorization or token refresh
- On MCP reload, reuse cached tokens silently

## Context

Discovered when trying to test toast notifications via MCP reload -- the
reload triggered browser OAuth popups for multiple servers simultaneously.
