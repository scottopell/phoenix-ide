# MCP Client -- Executive Summary

## Overview

Phoenix is an MCP client. It connects to MCP servers, discovers their tools,
and exposes them to the conversation runtime as `{server}__{tool}`. Servers are
reached over **stdio** (child process) or **HTTP (Streamable HTTP)**. The HTTP
transport adds an OAuth 2.1 authorization concern absent from stdio.

The stdio transport, tool exposure, reload reconciliation, and enable/disable
are established. The HTTP transport and its authorization are the build-out this
spec set scopes; the value driver is reaching OAuth-protected remote servers
natively, without the `mcp-remote` subprocess bridge.

## Status

| Requirement | Title | Status | Notes |
|---|---|---|---|
| REQ-MCP-001 | Transport-Tagged Config Discovery | Complete | `read_all_configs` merges entries first-seen-wins; `classify_config_entry` tags each as `Stdio` (a `command`) or `Http` (`type: "http"` + `url`, with generic `headers` and an optional `auth` credential), validates an optional positive `timeoutSeconds`, and defaults `tools/call` to five minutes. Reload restarts a server when this config value changes. |
| REQ-MCP-002 | Transport-Agnostic JSON-RPC Protocol | Complete | Protocol (initialize / paginated `tools/list` / `tools/call` / notification handling via `ServerMessageSink`) lives on `McpServer` over the `McpTransport` trait; a bounded per-server supervisor mailbox owns lifecycle state and epochs. Stdio calls execute sequentially in the supervisor; HTTP calls run concurrently and return epoch-tagged completions. |
| REQ-MCP-003 | Stdio Transport | Complete | `StdioTransport::spawn` / `McpServer::initialize` / `list_tools`; crash detection (`is_alive`, `TransportError::Disconnected` classification) + `respawn` in `mcp.rs`. Single-attempt calls retain late cancellation outcomes for connection recovery without tool replay; a paused-clock regression distinguishes invalidating and cancellation-safe transports. |
| REQ-MCP-004 | Streamable HTTP Transport | Complete | `HttpTransport` (`mcp/http.rs`): POST with the dual `Accept` pair, unary-JSON and SSE-stream response handling (`SseFramer`), `MCP-Protocol-Version` echoed after `initialize`, 202-with-empty-body accepted for notifications. |
| REQ-MCP-005 | HTTP Session Lifecycle | Complete | `Mcp-Session-Id` captured at initialize and echoed on later requests; one bounded best-effort DELETE on local retirement. Non-DELETE session 404 reinitializes. |
| REQ-MCP-006 | Server-Initiated Stream and Resumability | Complete | `ServerStream` (`mcp/http.rs`): a detached GET (`Accept: text/event-stream`, bearer + session + protocol-version) opened once `initialize` negotiates, feeding server-initiated messages to the shared `NotificationSink`; reconnect resumes via `Last-Event-ID` with capped backoff; aborted on `shutdown`/`Drop`. |
| REQ-MCP-007 | HTTP Connection Recovery | Partial | Process-local paced connection/handshake and refresh recovery implemented; validation below. Live fault-injection acceptance remains open. |
| REQ-MCP-008 | Static Token / Header Authentication | Complete | `HttpAuth::Static` (bearer or designated auth headers) attached to every request; generic `headers` ride every request without classifying the server. The 401-versus-OAuth routing distinction becomes observable when the OAuth flow exists (M3). |
| REQ-MCP-009 | OAuth 2.1 Authorization Discovery | Complete | `mcp/oauth.rs`: 401 challenge parsing → PRM (`resource_metadata` or the path-aware/root well-knowns, RFC 9728) → AS metadata via both RFC 8414 and OIDC discovery forms. A resource-advertised authorization server's self-declared issuer is accepted even when it differs from the fetch URL (`IssuerTrust::ResourceAdvertised`); a directly-named issuer is held to exact equality. |
| REQ-MCP-010 | Client Identity Acquisition | Complete | Cached registrations keyed by authorization server are reused; a pre-configured public client (Claude Code's top-level `oauth.clientId`, no secret — PKCE only) seeds the registration once discovery resolves the issuer; RFC 7591 DCR is the fallback. Phoenix hosts no Client ID Metadata Document, so that step resolves to nothing (logged at `debug`) and falls through to DCR. |
| REQ-MCP-011 | Authorization Code Flow with PKCE | Complete | Native flow in `mcp.rs` (`begin_oauth_flow` / `complete_oauth_authorization`): S256 PKCE (refused when not advertised), unguessable `state` bound to the pending flow, `iss` validation, RFC 8707 `resource` on both requests, and callback at `GET /api/mcp/oauth/callback`. `OAuthConfig` preserves Claude Code-compatible `oauth.scopes`; configured and challenge-required scopes are unioned, Protected Resource Metadata is the fallback when neither is present, and prior grants are always retained during re-authorization. |
| REQ-MCP-012 | Token Storage, Refresh, Invalidation, and Step-Up | Partial | Stored grants, rotation and owned authorization retained. Remote cleanup no longer requests credentials. Scope plans survive retired transports; transient lifecycle failures resume automatically. |
| REQ-MCP-013 | Authorization Status Surfaced to the UI | Partial | Connecting/recovering, last error, next retry and owned sign-in URL exposed; diagnostics polling covers background recovery. |
| REQ-MCP-014 | Tool Exposure and Live Resolution | Complete | `tool_definitions` / `create_mcp_tool_by_name`; live resolution via `ToolRegistryExecutor`. Schema-bound calls retain the selected supervisor and recheck its serving catalog before dispatch and recovery retry. Queued-call and respawn regressions verify rejection without replacement I/O. |
| REQ-MCP-015 | Config Reload Reconciliation | Complete | `reload_from_actor_configs` sends reconfigure/remove commands to per-server supervisors; epoch checks discard stale connect/recovery completions. The `PartialEq` comparison spans the `Stdio | Http` config variants and `timeoutSeconds`. |
| REQ-MCP-016 | Per-Server Enable/Disable | Complete | `disable_server` / `enable_server`; persisted in `mcp_disabled_servers` (`crates/phoenix-db/src/lib.rs`). |
| REQ-MCP-017 | Tool Call Cancellation and Error Surfacing | Complete | `McpTool::run` passes cancellation into `McpClientManager`; the per-server supervisor returns cancellation independently for each waiter and requires a new recovery epoch before stdio serves another call. `tools/call` honors `isError`. |
| REQ-MCP-018 | Connection Failure Visibility | Complete | Each supervisor snapshot retains `Failed` with `last_error` until a current-epoch publish succeeds or the server is removed. A server still awaiting auth is represented by a recovering snapshot with `pending_oauth_url`, not failed. `McpStatusPanel` renders the operator-facing states distinctly. |
| REQ-MCP-019 | Legacy HTTP+SSE Not Natively Supported | Complete | By decision; `legacy_sse_native = false`. Such servers use the `mcp-remote` stdio bridge. |
| REQ-MCP-020 | OAuth Redirect Origin Resolution | Complete | The redirect base is the canonical external origin: `PHOENIX_EXTERNAL_URL` override, else derived from the TLS host config (`ConfigSource::external_host`, first non-loopback host in order) + scheme + bind port (`resolve_external_origin`). Derived from trusted config, not request headers. A cached DCR client registered with a different `redirect_uri` is re-registered (`acquire_client_registration`; registrations carry the redirect_uri, phoenix-db migration 23). An all-interfaces bind that still resolves to loopback warns at startup and on every `unauthorized` status entry (`auth_redirect_warning`). A pre-configured client pinning a loopback `callbackPort` (its app allowlist Phoenix can't edit) uses an `http://localhost:<port>/callback` redirect and a loopback listener (`bind_loopback_listeners`/`run_loopback_redirect`, `OAuthRuntime::loopback_listeners`) — bound synchronously before the URL is surfaced (bind failure fails the flow), on both IP families, accepting until the flow resolves or 25 minutes expire — that 302-bounces the callback to the real route. An accepted callback with exactly one matching nonce retains thirty seconds for redirect completion without accepting new connections (`oauth_callback_near_deadline_retains_only_a_bounded_nonce_matched_grace`). |

## Automatic HTTP recovery implementation

ADR-230 is accepted following the owner's simplification decision. Local transport
activity must stop before replacement; remote DELETE is one bounded best-effort
attempt and never starts credential recovery. Connection timers are process-local.
Durability for credentials, local credential-removal intent and accepted Slack
work remains unchanged. No database schema changes are introduced.

Implementation validation covers:

| Scenario | Required observation |
|---|---|
| Initial HTTP outage | Recovery reaches ready without a caller, status reader, reload or process restart. |
| DELETE 401/403/404/405/429/5xx | One attempt; local retirement succeeds; no cleanup-only OAuth. |
| Token endpoint throttle or connection throttle | Preserve Retry-After, publish retry status; unchanged reload joins the same deadline. |
| Refresh succeeds, later handshake fails | Reuse the grant; rejected fresh grant requests owned authorization. |
| Rotated grant persistence fails | Retry the retained response without another grant exchange. |
| Reconfiguration/removal races with scheduled recovery | Cancel obsolete retries and reject stale callbacks/completions. |
| Denied/unstartable authorization | Explicit unchanged-config retry retains scope union and challenge independently of old transport. |
| Local credential deletion fails | Keep durable removal intent and finish it on reconstructed startup. |
| Uncertain dispatched one-shot call | Restore service for later operations; never replay the original operation. |
| Slack connection outage | Report current connection state separately from prior scan success. |

The specification-only [review record](../../docs/reviews/2026-11-06-http-recovery-spec-review.md)
records the initial draft and the subsequent simplification. Local qualification evidence
is recorded there: 180 MCP, 54 application Slack, 27 Slack database and 16 UI
regressions pass, along with Clippy, TypeScript, lint, formatting and spec checks. Live fault injection and
production deployment are separate, unperformed gates; process startup alone
cannot establish acceptance or authorize a database-lineage conversion.

## Milestones

The fixed-port local sign-in window is 25 minutes. Callback/status handling
expires stopped listeners, clears their authorization URLs, and retains the
supervisor's reload retry plan. Regression tests cover callbacks after five
minutes, retries within the window, expiry, and removal of dead sign-in links.
This deadline does not alter provider code or token expiry.

This spec set is M0 of the native HTTP MCP build-out. The milestones:

- **M1** (done) -- Extract the `McpTransport` trait; turn `McpServerConfig`
  into a `Stdio | Http` enum. No behavior change (REQ-MCP-002, REQ-MCP-015).
- **M2** (done) -- Streamable HTTP transport substrate with static/no auth
  (REQ-MCP-001, -004, -005, -007, -008). Prerequisite for OAuth, not an
  independent release.
- **M3** (done) -- OAuth 2.1, the value driver. First releasable unit = M2 + M3
  (REQ-MCP-009 .. -013).
- **M4** (done) -- Server-initiated SSE stream + resumability (REQ-MCP-006).
- **M5** (done) -- UI / config / ops polish: connection-failure visibility
  (REQ-MCP-018) and the consolidated OAuth redirect origin (REQ-MCP-020).

## Design Decisions

- **Legacy HTTP+SSE is not implemented natively** (REQ-MCP-019). Streamable HTTP
  only; `mcp-remote` covers legacy servers during their decline.
- **OAuth tokens are stored plaintext in SQLite** (REQ-MCP-012), consistent with
  existing operator-state storage; the database file's on-disk protection is the
  trust boundary.
- **`mcp-remote` is retained as a transition fallback.** Native HTTP does not
  remove the stdio path; an HTTP server can still be configured as
  `npx mcp-remote <url>`.
- **Static auth is not an independent milestone.** It costs nothing on top of
  the M2 transport, but OAuth (M3) is the deliverable users feel.
- **The OAuth redirect origin is derived from the TLS host config, not request
  headers** (REQ-MCP-020). The reachable domain an operator already sets for the
  certificate is the single source of truth for the callback origin, so a
  self-hosted remote deployment needs no separate knob. Deriving from trusted
  config rather than the `Host`/`Forwarded` headers removes the redirect-target
  injection surface, so no trusted-proxy or origin-allowlist machinery exists.
  `PHOENIX_EXTERNAL_URL` overrides for proxy-terminated TLS or manual certs.

## Allium Spec

`mcp.allium` models the supervisor ownership, process-local HTTP recovery episode,
credential transitions and uncertain-dispatch boundary. Scope/challenge plans
belong to the server recovery owner independently of transport retirement.
`HttpSessionDeletedOnShutdown` models attempted remote retirement, not guaranteed
server deletion. Browser-flow failure remains an explicit-action boundary.

Strict cleanup-only authorization, deferred removal authorization, queued
configuration waiting for remote cleanup and the associated UI were removed.
Their regression retirement and replacement coverage are recorded in the review
record. Ordinary epoch/schema fencing, local quiescence, rotating-token persistence,
callback nonce checks and local removal-intent tests remain.
