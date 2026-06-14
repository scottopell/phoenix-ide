Re-register the OAuth client (RFC 7591 DCR) when the redirect base changes.

The OAuth redirect base is now the canonical external origin derived from the
TLS host config (REQ-MCP-020). A cached DCR registration (keyed by
authorization server in mcp_oauth_registrations) bakes in the redirect_uri it
was registered with, but that column does not exist yet. If the operator
changes the reachable domain (PHOENIX_TLS_HOSTS / PHOENIX_EXTERNAL_URL) after a
registration was made, the authorization request's redirect_uri no longer
matches the registered one and the authorization server rejects it.

## What to do
- Add a redirect_uri column to mcp_oauth_registrations (phoenix-db migration)
  and persist it on registration.
- At pending-flow construction, compare the resolved redirect base to the
  cached registration's; if different, re-register (RFC 7591) and persist.
- Until then the failure surfaces as a `failed`/`unauthorized` server whose
  last_error names the redirect mismatch (REQ-MCP-018), so it is at least
  legible -- this task makes recovery automatic.
