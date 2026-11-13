# ADR-251: Explicit loopback OAuth callback URLs reuse the fixed-port listener

- **Status:** Accepted
- **Date:** 2026-11-13
- **Affects:** REQ-MCP-020

## Context

Phoenix's shared Slack OAuth application needs a fixed
`http://localhost:<port>/callback` address. Its `oauth.callbackPort` configuration
starts a local listener that forwards the browser to Phoenix's normal callback
handler. Snowflake's built-in `LOCAL_APPLICATION` identity accepts an IPv4
loopback root address in the tested account, but rejects that Slack
callback shape. The existing code exchange, token storage, and refresh flow
support a preconfigured public client.

## Options considered

1. Register another Snowflake application that accepts Phoenix's callback.
   This avoids a code change but requires a Snowflake administrator.
2. Add separate callback-host and callback-path settings beside callbackPort.
   This supports the address, but reconstructs an OAuth URI from several knobs.
3. Accept an explicit local callback URL and reuse the existing listener.
   This keeps the provider's exact registered URI in one configuration value.

## Decision

Add `oauth.callbackUrl` for a preconfigured public client, mutually exclusive
with `oauth.callbackPort`. Validate it as HTTP on `localhost`, `127.0.0.1`, or
`[::1]`, with a nonzero resolved port, no credentials, query, fragment,
whitespace, control characters, or backslashes. Retain its exact text for the
authorization URL and token request. Bind literal callbacks on that IP family;
a successful bind on the other family cannot receive the specified callback.

The existing `callbackPort` shape remains supported and produces exactly
`http://localhost:<port>/callback`, because the working Slack application relies
on that registered address. Both inputs resolve to one validated callback type;
neither introduces a second token path or a separate process.

## Consequences

- **Positive:** Providers with different registered local paths can use the
  existing OAuth completion and token lifecycle.
- **Negative:** Explicit callback URLs require validation, and the account's
  acceptance of a client ID still needs live authorization testing.
- **Neutral:** This is local-browser sign-in. It does not provide remote callback
  forwarding or confidential-client authentication.

## References

- [MCP requirements](../mcp/requirements.md), REQ-MCP-020.
- `LoopbackCallback`, `begin_oauth_flow`, and `bind_loopback_listeners` in
  `crates/phoenix-mcp/src/lib.rs`.
- [Snowflake OAuth for local applications](https://docs.snowflake.com/en/user-guide/oauth-local-applications).
