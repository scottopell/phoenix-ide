# ADR-229: HTTP connection recovery retries under existing ownership

- **Status:** Proposed
- **Date:** 2026-11-06
- **Affects:** REQ-MCP-003/007/012/013/018; REQ-SG-017

## Context

A transient HTTP failure can leave a retained, non-callable transport whose
session deletion failed. The configured service then has no callable tools. If
the credential expires before an explicit reload, deletion itself can require
OAuth recovery. Retaining cleanup ownership prevents overlapping replacement
sessions, but ownership without another scheduled attempt cannot restore service.

The MCP contract permits automatic transient refresh retries, while ordinary
cleanup failures can stop for explicit retry. Background Slack discovery depends
on the same connection and cannot repair that distinction by polling for tools.

## Options considered

1. Keep explicit reload/restart as recovery. Smallest implementation, but an
   unattended integration can remain unavailable after the network recovers.
2. Have the Slack gateway reload MCP or restart Phoenix after failed scans.
   Locally convenient, but duplicates lifecycle ownership, affects other MCP
   users and can bypass cleanup, credential and uncertain-operation boundaries.
3. Extend configured HTTP lifecycle recovery under the existing per-server
   owner. Retry typed transient failures, route cleanup authorization into OAuth,
   and expose current recovery through native status. This requires correct
   fencing and pacing, but keeps one owner of sessions and credentials.
4. Add durable recovery jobs and remote-session reconstruction. This would add
   storage, migration, timer recovery and remote cleanup guarantees beyond the
   actual need to recover an available configured service in one running process.

## Decision

Propose option 3. Use the existing MCP supervisor, recovery epoch, credential
mutation fence and retained transport. A process-local retry episode continues
through transient connection, cleanup, handshake, refresh and recovered-token
persistence failures. A cleanup 401 can admit OAuth recovery even when transport
failure started the episode. Never infer policy from formatted error strings.

Five-second initial backoff grows to sixty seconds, with bounded jitter and
server-directed deadlines as a lower bound. Retry eligibility has no attempt-count
cutoff, but every network operation retains a timeout and ownership cancellation.
Permanent failures, local quiescence failures, denied authorization and failed
authorization setup remain explicit-action boundaries.

The scope is configured HTTP connection establishment/recovery, including old
cleanup needed for queued configuration. Automatic removal cleanup and stdio
respawn loops are not added. Existing transient-refresh removal behavior remains.
Retry schedules and HTTP session identities remain volatile. Existing configured
startup and credential removal recovery retain their own contracts.

Native MCP status exposes the active phase, most recent failure and next retry.
Slack derives its connection health from that status and keeps the last completed
scan distinct. The gateway adds no reconnect controller, health history table,
alert outbox or changes to single-attempt notifications and acknowledgments.

## Consequences

- **Positive:** An unattended configured HTTP integration can recover after
  network loss and token expiry without a caller, reload or process restart.
- **Positive:** Connection recovery never authorizes tool replay or bypasses
  gateway request identity, authenticated-account verification or call budgets.
- **Negative:** Transient versus permanent failure classification must survive
  session cleanup. Recovery timers and completions need fencing against queued
  configuration, removal and shutdown. A falsely transient permanent outage can
  continue producing bounded background traffic until configuration is corrected.
- **Negative:** Native status needs new recovery diagnostics, and tests that
  expect transient cleanup to remain terminal need deliberate revision.
- **Neutral:** No new database tables, retry history, cross-process session
  reconstruction, process watchdog or prototype conversion is required. Deployment
  and cross-version qualification remain separate from this recovery decision.

## References

- [MCP requirements](../mcp/requirements.md)
- [MCP lifecycle](../mcp/mcp.allium)
- [Slack discovery](../slack-gateway/discovery.allium)
- [Compatibility policy](034_compatibility-guarantees-are-explicit-and-data-aware.md)
- [Credential removal scope](086_mcp-credential-removal-has-durable-intent.md)
- `McpClientManager::recover_actor`, `McpClientManager::retry_oauth_cleanup_owned`,
  `Supervisor::stop_server`, `HttpTransport::shutdown`
