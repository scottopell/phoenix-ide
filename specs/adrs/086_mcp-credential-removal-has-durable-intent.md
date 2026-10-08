# ADR-086: MCP credential removal has durable intent

- **Status:** Accepted
- **Date:** 2026-10-07
- **Affects:** REQ-MCP-012; OAuthTokenRemoval, TokenHasOAuthOwner

## Context

Authenticated session cleanup and local credential deletion cross a network and
a database boundary. A transient deletion failure can leave a credential stored
after transport cleanup succeeds. An in-memory supervisor cannot resume that
deletion after restart when configuration no longer contains the server name.

## Options considered

1. Keep only an in-memory failed removal owner: preserves live retry, but loses
   the owner after restart.
2. Delete every credential absent from discovered configuration: needs no marker,
   but cannot distinguish admitted removals from incomplete configuration discovery.
3. Persist removal intent independently of the grant and complete both atomically:
   identifies exactly the admitted removals that startup can resume.

## Decision

Choose option 3. Record the server name before admitting removal cleanup. Keep
the supervisor non-callable and visible until credential deletion and intent
completion commit together. A separate relational row survives grant rejection
and rotation. Startup and reload complete recorded credential removals for names
still absent from configuration. Re-adding a name cancels the intent before
reconnecting and preserves its grant.

The recovery guarantee covers credentials and removal intent. HTTP session IDs
remain volatile; this mechanism does not reconstruct or delete a lost remote
session after process restart. Its remaining lifetime belongs to the remote service.

## Consequences

- **Positive:** Failed deletion remains actionable during the live process and
  after restart, without inferring removals from arbitrary missing config entries.
- **Negative:** Removal admission requires a durable write, and reconciliation
  must check persisted intents. Failed writes block completion.
- **Neutral:** Shared authorization-server registrations remain independent.
  Migration 118 creates the intent table without inventing intent for existing grants.

## References

- [Compatibility policy](034_compatibility-guarantees-are-explicit-and-data-aware.md)
- [MCP requirements](../mcp/requirements.md)
- `OAuthStore`, `McpClientManager::complete_server_removal`,
  `Database::complete_mcp_oauth_removal`
