Surface the OAuth redirect loopback-on-remote diagnostic in /api/mcp/status.

When an all-interfaces bind resolves the OAuth redirect base to loopback
(no reachable name configured, REQ-MCP-020), Phoenix warns at startup. Also
surface this in the status API so an operator sees it in the panel next to the
doomed Authorize link, not only in logs.

## What to do
- Thread the startup-computed warning onto the manager (OAuthRuntime) when the
  loopback-on-remote condition holds.
- Add an optional `auth_redirect_warning` to McpServerStatus, populated on
  unauthorized entries.
- Render it under the auth banner in McpStatusPanel.

The startup WARN already covers the operator signal; this is the in-UI
legibility enhancement.
