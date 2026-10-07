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
| REQ-MCP-003 | Stdio Transport | Complete | `StdioTransport::spawn` / `McpServer::initialize` / `list_tools`; crash detection (`is_alive`, `TransportError::Disconnected` classification) + `respawn` in `mcp.rs`. |
| REQ-MCP-004 | Streamable HTTP Transport | Complete | `HttpTransport` (`mcp/http.rs`): POST with the dual `Accept` pair, unary-JSON and SSE-stream response handling (`SseFramer`), `MCP-Protocol-Version` echoed after `initialize`, 202-with-empty-body accepted for notifications. |
| REQ-MCP-005 | HTTP Session Lifecycle | Complete | `Mcp-Session-Id` captured at `initialize` and echoed on every request; DELETE on shutdown (`HttpTransport::shutdown`); 404 → `TransportError::SessionExpired` → re-initialize before one retry. |
| REQ-MCP-006 | Server-Initiated Stream and Resumability | Complete | `ServerStream` (`mcp/http.rs`): a detached GET (`Accept: text/event-stream`, bearer + session + protocol-version) opened once `initialize` negotiates, feeding server-initiated messages to the shared `NotificationSink`; reconnect resumes via `Last-Event-ID` with capped backoff; aborted on `shutdown`/`Drop`. |
| REQ-MCP-007 | HTTP Connection Recovery | Complete | `McpServer::should_reestablish`: HTTP recovers from `Disconnected`/`Timeout`/`SessionExpired` by rebuilding the client + handshake, distinct from the stdio respawn (which recovers only from `Disconnected`). |
| REQ-MCP-008 | Static Token / Header Authentication | Complete | `HttpAuth::Static` (bearer or designated auth headers) attached to every request; generic `headers` ride every request without classifying the server. The 401-versus-OAuth routing distinction becomes observable when the OAuth flow exists (M3). |
| REQ-MCP-009 | OAuth 2.1 Authorization Discovery | Complete | `mcp/oauth.rs`: 401 challenge parsing → PRM (`resource_metadata` or the path-aware/root well-knowns, RFC 9728) → AS metadata via both RFC 8414 and OIDC discovery forms. A resource-advertised authorization server's self-declared issuer is accepted even when it differs from the fetch URL (`IssuerTrust::ResourceAdvertised`); a directly-named issuer is held to exact equality. |
| REQ-MCP-010 | Client Identity Acquisition | Complete | Cached registrations keyed by authorization server are reused; a pre-configured public client (Claude Code's top-level `oauth.clientId`, no secret — PKCE only) seeds the registration once discovery resolves the issuer; RFC 7591 DCR is the fallback. Phoenix hosts no Client ID Metadata Document, so that step resolves to nothing (logged at `debug`) and falls through to DCR. |
| REQ-MCP-011 | Authorization Code Flow with PKCE | Complete | Native flow in `mcp.rs` (`begin_oauth_flow` / `complete_oauth_authorization`): S256 PKCE (refused when not advertised), unguessable `state` bound to the pending flow, `iss` validation, RFC 8707 `resource` on both requests, and callback at `GET /api/mcp/oauth/callback`. `OAuthConfig` preserves Claude Code-compatible `oauth.scopes`; configured and challenge-required scopes are unioned, Protected Resource Metadata is the fallback when neither is present, and prior grants are always retained during re-authorization. |
| REQ-MCP-012 | Token Storage, Refresh, Invalidation, and Step-Up | Complete | `mcp_oauth_registrations` + `mcp_oauth_tokens` (phoenix-db migration 22, plaintext); bearer on every request via the shared cell; silent restore (resource-matched, unexpired-or-refreshable); refresh with rotation persisted; refresh rejection discards and re-prompts; 403 `insufficient_scope` steps up with the scope union while the triggering call waits on the supervisor recovery epoch. |
| REQ-MCP-013 | Authorization Status Surfaced to the UI | Complete | `GET /api/mcp/status` carries each server's `state` (`ready`/`unauthorized`/`failed`), `transport`, `auth`, and the native flow's structured `pending_oauth_url` (the stdio `mcp-remote` path still feeds the same map from its stderr drain). |
| REQ-MCP-014 | Tool Exposure and Live Resolution | Complete | `tool_definitions` / `create_mcp_tool_by_name`; live resolution via `ToolRegistryExecutor`. HTTP servers ride this unchanged. |
| REQ-MCP-015 | Config Reload Reconciliation | Complete | `reload_from_actor_configs` sends reconfigure/remove commands to per-server supervisors; epoch checks discard stale connect/recovery completions. The `PartialEq` comparison spans the `Stdio | Http` config variants and `timeoutSeconds`. |
| REQ-MCP-016 | Per-Server Enable/Disable | Complete | `disable_server` / `enable_server`; persisted in `mcp_disabled_servers` (`crates/phoenix-db/src/lib.rs`). |
| REQ-MCP-017 | Tool Call Cancellation and Error Surfacing | Complete | `McpTool::run` passes cancellation into `McpClientManager`; the per-server supervisor returns cancellation independently for each waiter and requires a new recovery epoch before stdio serves another call. `tools/call` honors `isError`. |
| REQ-MCP-018 | Connection Failure Visibility | Complete | Each supervisor snapshot retains `Failed` with `last_error` until a current-epoch publish succeeds or the server is removed. A server still awaiting auth is represented by a recovering snapshot with `pending_oauth_url`, not failed. `McpStatusPanel` renders the operator-facing states distinctly. |
| REQ-MCP-019 | Legacy HTTP+SSE Not Natively Supported | Complete | By decision; `legacy_sse_native = false`. Such servers use the `mcp-remote` stdio bridge. |
| REQ-MCP-020 | OAuth Redirect Origin Resolution | Complete | The redirect base is the canonical external origin: `PHOENIX_EXTERNAL_URL` override, else derived from the TLS host config (`ConfigSource::external_host`, first non-loopback host in order) + scheme + bind port (`resolve_external_origin`). Derived from trusted config, not request headers. A cached DCR client registered with a different `redirect_uri` is re-registered (`acquire_client_registration`; registrations carry the redirect_uri, phoenix-db migration 23). An all-interfaces bind that still resolves to loopback warns at startup and on every `unauthorized` status entry (`auth_redirect_warning`). A pre-configured client pinning a loopback `callbackPort` (its app allowlist Phoenix can't edit) uses an `http://localhost:<port>/callback` redirect and a loopback listener (`bind_loopback_listeners`/`run_loopback_redirect`, `OAuthRuntime::loopback_listeners`) — bound synchronously before the URL is surfaced (bind failure fails the flow), on both IP families, accepting until the flow resolves — that 302-bounces the callback to the real route. |

## Milestones

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

OAuth recovery claims its epoch before authenticated teardown. The supervisor
retains the old transport privately, refreshes or awaits re-authorization, and
uses the new bearer for cleanup before connecting a replacement. Coverage in
`phoenix-mcp` includes `tool_call_401_refreshes_and_replays_the_call`,
`concurrent_oauth_recovery_refreshes_once_and_cleans_up_with_fresh_bearer`,
`transient_oauth_refresh_retries_before_session_cleanup`,
`refresh_rejection_discards_token_and_reprompts`, and
`oauth_refresh_keeps_failed_delete_owned_and_blocks_replacement`. Scope step-up
uses the same retained ownership; its end-to-end regression requires the
upgraded bearer on DELETE. The gated supervisor test
`oauth_callback_cleanup_and_restart_cannot_supersede_queued_reload` covers a
reload queued while callback cleanup is blocked.
`oauth_refresh_mutations_are_serialized_with_reload_for_success_and_rejection`
gates token responses and covers both initial refresh and background retry,
ensuring newer credentials and cancelled flows survive the old recovery.
`replacement_oauth_connection_cannot_publish_a_flow_after_reload` gates the
replacement handshake while a configuration reload is queued, then verifies
that the new endpoint is ready without an obsolete OAuth token or prompt.
`failed_removal_preserves_oauth_cleanup_owner_for_callback` covers a callback
after failed removal and proves it completes removal without reconnecting.
`readding_a_server_restores_reconnect_intent_after_failed_removal` covers an
explicitly restored configuration. Supervisor regressions cover cancellation before cleanup
and preservation of unrelated stale transport credentials.
`oauth_prompt_binds_cleanup_owner_before_recovery_returns` verifies prompt
ownership and authenticated cleanup for an immediate callback after rejected
refresh or scope step-up preparation.
`queued_removal_uses_the_bearer_recovered_by_refresh` covers removal queued
during token refresh. `slow_connection_does_not_block_another_servers_oauth_refresh`
verifies that one blocked handshake does not serialize another server's recovery.
`removed_oauth_owner_is_forgotten_after_denial_cleanup_succeeds` covers denial
with successful and failed removal cleanup, including re-authorization after
re-adding a denied removal while DELETE still requires a fresh bearer.
`readding_removed_owner_reconnects_after_callback_token_delete_failure` covers
re-addition after a token-store deletion error.
`oauth_claim_quiesces_http_stream_without_deleting_the_session` verifies that
retained sessions stop their server-initiated stream before credential recovery.
`denied_step_up_can_reauthorize_on_unchanged_reload` covers a denied scope
upgrade, preserved scopes on explicit reload, stale callback rejection, and
authenticated cleanup before replacement publication.
`oauth_refresh_persistence_retry_does_not_repeat_rotating_grant` covers repeated
store-write failures after rotation, one grant exchange, and fresh authenticated
cleanup after the response is persisted.
`configuration_invalidates_unpersisted_refresh_even_when_store_lookup_fails`
verifies that local lookup errors cannot retain a response invalidated by config.
`oauth_quiescence_failure_settles_failed_and_retains_cleanup` covers a visible
failed state and retained teardown ownership when stream quiescence fails.
`oauth_failure_takes_over_failed_transport_cleanup_from_same_epoch` covers
transport recovery winning leadership before OAuth recovery, with expired
DELETE followed by refreshed DELETE and a ready replacement.
`rejected_refresh_with_unstartable_authorization_settles_failed` verifies
visible failure without an endless retry, followed by explicit authorization retry.
`unstartable_step_up_preserves_scope_union_for_explicit_retry` covers the
equivalent scope-upgrade setup failure without losing the requested grants.
`removal_preserves_transient_refresh_until_cleanup_or_readdition` covers removal
between retries, removal after successful refresh, restored reconnect intent
on re-addition, and cleanup-only authorization after the grant is rejected.
`denied_authorization_retry_preserves_challenge_directed_discovery` requires a
nonstandard metadata URI across denial and explicit reload.
`changed_configuration_waits_for_transient_oauth_cleanup` verifies both refresh
success and rejected-grant authorization before applying a new resource.
`changed_configuration_replaces_pending_cleanup_authorization` covers fresh
nonce and preserved scopes when a pending or denied flow is reconfigured.
`replacement_handshake_reauthorization_is_owned_and_visible` verifies owned,
visible authorization when replacement initialization rejects the refreshed grant.
`replacement_tools_list_401_and_failed_delete_can_reauthorize` covers a
session-bearing replacement's tools/list 401, failed DELETE, denial, explicit
retry, and fresh authenticated cleanup. `startup_tools_list_401_and_failed_delete_refresh_silently`
covers the same handshake failure with successful and transient startup refresh.
`refreshed_handshake_cleanup_preserves_auth_cause_without_repeating_grant`
covers a rejected tools/list after one silent grant and preserved authorization
provenance through fresh-recovery teardown failure.
`refreshed_handshake_with_successful_teardown_does_not_repeat_grant` covers
initialize and tools/list rejection after the first refresh when cleanup succeeds,
with one grant followed by an owned prompt preserving prior granted scopes
and successful browser authorization.

Behavioral specification: `specs/mcp/mcp.allium`

Models the per-server `ConnState` lifecycle
(`connecting → ready`, with `reconnecting` / `unauthorized` / `failed`
recovery), the `OAuthPhase` authorization sub-lifecycle
(`discovering → registering → awaiting_user → authorized`, plus `refreshing`),
the `McpServer` / `OAuthRegistration` / `OAuthToken` entities, and invariants
binding session ids and tokens to the HTTP/OAuth servers that own them.
</content>
