M5 of the native HTTP MCP build-out (umbrella: tasks/58024). Two parts:

## Part A -- Connection-failure visibility (REQ-MCP-018)

Failed MCP servers currently vanish from GET /api/mcp/status (connect_one
logs "Skipping MCP server" and returns None). Retain them with their error so
a misconfigured server is distinguishable from one merely awaiting auth.

- McpServerStatus gains `state` (ready|unauthorized|failed), `last_error`,
  `transport` (stdio|http), `auth` (none|static|oauth).
- A `failed_servers` map parallel to `pending_oauth_urls`, populated on
  connect/handshake/OAuth-flow failure and reestablish-drop, cleared on
  successful reconnect and on removal.
- status() unions connected/unauthorized/failed, deduped.
- Reload retries failed + pending.
- McpStatusPanel renders ready/unauthorized/failed distinctly (red error
  banner vs yellow auth banner) + transport/auth inline.

## Part B -- Consolidated OAuth redirect origin (REQ-MCP-020)

The OAuth redirect base is the canonical external origin, derived from the
TLS host config rather than a separate knob: scheme from TLS presence, host =
first non-loopback PHOENIX_TLS_HOSTS entry (else bind host), port from bind.
PHOENIX_EXTERNAL_URL is the override (proxy-terminated TLS, manual TLS,
multi-host disambiguation). No request-header derivation, no trust/allowlist
machinery, no cert-SAN parsing.

- canonical_external_origin() helper shared by TLS reporting + OAuth redirect.
- Re-register DCR when the redirect base differs from the cached registration.
- Loopback-on-remote diagnostic on the unauthorized status entry: redirect
  host is loopback but a non-loopback TLS host is configured.

## Spec/docs

REQ-MCP-018 -> Complete; add REQ-MCP-020; reconcile mcp.allium status
projection; executive M5 -> done. Document the PHOENIX_TLS_HOSTS story +
PHOENIX_EXTERNAL_URL escape hatch + HTTP MCP config schema.
