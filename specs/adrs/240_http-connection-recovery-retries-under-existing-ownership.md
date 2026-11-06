# ADR-240: HTTP connection recovery retries under existing ownership

- **Status:** Accepted
- **Date:** 2026-11-06
- **Affects:** REQ-MCP-003/005/007/012/013/018; REQ-SG-017

## Context

A transient HTTP failure can leave a retained, non-callable transport whose
session deletion failed. The configured service then has no callable tools. If
the credential expires before an explicit reload, deletion itself can require
OAuth recovery. Retaining cleanup ownership prevents overlapping replacement
sessions, but ownership without another scheduled attempt cannot restore service.

Background Slack discovery depends on the same connection and cannot repair
it by polling for tools. Requiring successful remote DELETE before replacement
also couples availability to cleanup-only OAuth flows, retained transports and
queued configuration. The owner approved removing that guarantee while retaining
durability for accepted work and local credential-removal intent.

## Options considered

1. Keep manual reload/restart as recovery. Smallest change, but unattended work
   can remain unavailable after the network recovers.
2. Add Slack-owned reconnect and alert loops. This duplicates shared lifecycle
   ownership and affects credential and uncertain-operation boundaries.
3. Retry configured HTTP lifecycle operations in the existing supervisor, with
   best-effort remote retirement. Keep timers process-local and expose current
   status. This restores service without making remote cleanup an auth workflow.
4. Add durable recovery jobs, remote-session reconstruction and complete outage
   capture. Storage, timer recovery and cross-version semantics exceed the need.

## Decision

Use option 3. Stop local transport activity before replacement. Attempt remote
DELETE once, for at most five seconds, using available credentials. Treat 404 as
already absent and 405 as unsupported; log other failures, retire the local session
and continue. Do not refresh a grant, request browser authorization or retry solely
to delete an obsolete session. A remote session may remain until server expiration.
Local quiescence failure remains a hard replacement boundary.

The existing per-server supervisor, epoch and credential mutation fence own
configured HTTP recovery. Typed transient transport/handshake, token-refresh and
recovered-token persistence failures use process-local backoff from five to sixty
seconds, with nonnegative jitter inside the cap and valid Retry-After as a lower
bound. Unchanged reloads join recovery. Successful readiness resets backoff.
Permanent failures and denied/unstartable authorization require explicit action.
A rejected freshly recovered grant requests authorization rather than looping
through refresh. Retain scope/challenge plans on the server owner independently
of retired transports. Preserve a rotated refresh response across local persistence
retry, without exchanging its predecessor again.

Removal cancels obsolete authorization. Existing durable local credential-removal
intent remains governed by ADR-086. Credentials, accepted Slack requests, thread
bindings, conversations and reply obligations retain their existing persistence.
Connection recovery never authorizes replay of an uncertain dispatched operation.
A process restart reconstructs configured connections from stored credentials;
it does not restore remote sessions or retry timers.

Native status exposes current connection state, last failure and next retry.
Slack derives connection health from it and reports the last completed scan and
an old-window warning. It adds no reconnect controller, phase UI, health-history
store or alert outbox. The four-calendar-date discovery window and single-attempt
notifications/acknowledgments remain unchanged.

## Consequences

- **Positive:** An unattended configured HTTP integration can recover after
  network loss and token expiry without a caller, reload or process restart.
- **Positive:** Connection recovery never authorizes tool replay or bypasses
  gateway request identity, authenticated-account verification or call budgets.
- **Negative:** Failed remote deletion can leave a remote session until the
  server expires it. No guarantee is made about that expiration time.
- **Negative:** Typed retry classification, ownership fencing and pacing need
  targeted regression coverage. A falsely transient failure can keep producing
  bounded traffic until the configuration is corrected.
- **Neutral:** No new database tables, retry history, cross-process session
  reconstruction, process watchdog or prototype conversion is required. Deployment
  and cross-version qualification remain separate from this recovery decision.

## References

- [MCP requirements](../mcp/requirements.md)
- [MCP lifecycle](../mcp/mcp.allium)
- [Slack discovery](../slack-gateway/discovery.allium)
- [Compatibility policy](034_compatibility-guarantees-are-explicit-and-data-aware.md)
- [Credential removal scope](086_mcp-credential-removal-has-durable-intent.md)
- `McpClientManager::recover_actor`, `McpClientManager::spawn_connect_retry`,
  `Supervisor::stop_server`, `HttpTransport::shutdown`
