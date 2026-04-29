---
created: 2026-04-29
priority: p3
status: ready
artifact: ui/src/components/McpStatusPanel.tsx
---

Surface MCP server connection failures in the UI instead of silently hiding the panel.

Currently when an MCP server background connect fails (mcp-remote spawn error, OAuth timeout, transport error, etc.), the backend logs a `Skipping MCP server: {e}` warn and the server name vanishes from /api/mcp/status. The UI panel disappears entirely if there are no other servers, leaving the user with no signal that something went wrong — only the toast from the reload click, which expires in 3s.

Acceptance:
- Failed servers appear in /api/mcp/status with a `last_error: string` field (or similar) carrying the warn message.
- McpStatusPanel renders failed servers with a distinct error banner (red, vs. the yellow auth-needed banner) including the error message.
- The reload (⟳) button retries the connection. Successful retry clears the error.
- Backend keeps the failed entry in some `failed_servers: HashMap<String, String>` map (name → error), parallel to `pending_oauth_urls`. Cleared on successful reconnect, populated on connect_one Err.

This complements the existing OAuth-pending banner. Together: pending OAuth (yellow, has URL), connection failed (red, has error), connected (normal panel row).
