# Unify bounded graceful shutdown across the HTTP and HTTPS server paths

## Problem

The plain-HTTP server path (`main.rs`) and the HTTPS path
(`tls::serve_https`) each implement "bounded graceful shutdown"
independently, with different mechanics. That is a parallel
representation of one concept — and it is exactly the smell that
produced a P1 on PR #117 / task 02708:

1. First the HTTP path shipped with *no* bound at all while the HTTPS
   path already had one (`SHUTDOWN_GRACE`) — a stuck SSE stream could
   pin the process past a deploy.
2. The follow-up bound on the HTTP path was then itself wrong (the
   `tokio::time::timeout` clock started at startup instead of at the
   shutdown signal, so the non-TLS server stopped itself every 30s).
   Caught by the Codex review on PR #117.

Both instances are fixed, but the two paths can still drift again
because the deadline is enforced in two separate places.

## Current state

- `main.rs`: `axum::serve(listener, app).with_graceful_shutdown(<oneshot>)`
  spawned as a task; `hot_restart::shutdown_signal()` awaited separately;
  `tokio::time::timeout(tls::SHUTDOWN_GRACE, …)` bounds the post-signal
  drain; the task is aborted on elapse.
- `tls::serve_https`: a hand-rolled `accept()` loop in `tokio::select!`
  against `shutdown_signal()`; per-connection TLS handshake, `set_nodelay`,
  ALPN logging; `hyper_util::server::graceful::GracefulShutdown`; then
  `tokio::select! { graceful.shutdown() / sleep(SHUTDOWN_GRACE) }`.

`SHUTDOWN_GRACE` is already a single shared `pub(crate)` const — keep it.

## Goal

The `SHUTDOWN_GRACE` deadline should be applied in exactly one place
that both paths route through, so the two server paths cannot diverge
on the shutdown contract again.

## Design notes

The HTTPS path hand-rolls its accept loop because it needs hooks
`axum::serve` does not expose (TLS handshake, ALPN logging, per-conn
`set_nodelay`). So the accept-loop divergence is inherent and
acceptable — the piece worth unifying is the *shutdown bounding*, not
the accept loop.

- **Option A (recommended, minimal):** factor a shared helper —
  e.g. `run_bounded(server_future, shutdown_signal) -> Result<…>` —
  that runs a server future, forks the signal into the server's
  graceful hook, and bounds the post-signal drain by `SHUTDOWN_GRACE`.
  Both `main.rs` and `serve_https` call it. The bound lives in one
  function; the accept loops stay path-specific.
- **Option B (larger):** a single `serve(listener, app, tls: Option<…>)`
  entry point that branches on TLS internally but bounds shutdown once.
  True single entry point, bigger change.

Recommend Option A. Reassess against task 24685
(`ConversationRuntime` graceful shutdown) — if that lands an app-level
shutdown `CancellationToken`, the helper here should consume the same
token rather than calling `shutdown_signal()` itself.

## Acceptance

- The `SHUTDOWN_GRACE` deadline is enforced in exactly one place; both
  the HTTP and HTTPS paths route through it.
- No behavior change: a server with no signal runs indefinitely; on
  SIGTERM/SIGINT/SIGHUP it drains, and a stuck connection (e.g. an SSE
  stream) is force-closed after `SHUTDOWN_GRACE` on both paths.
- Manual: confirm the plain-HTTP server stays up well past 30s with no
  signal, and exits within `SHUTDOWN_GRACE` on SIGTERM with a stuck
  SSE client attached.
- `./dev.py check` green.

## References

- PR #117 — task 02708 (bounded HTTP shutdown) and the Codex P1 review
  that caught the startup-clock bug
- `crates/phoenix-ide/src/main.rs` (HTTP path), `crates/phoenix-ide/src/tls.rs` (`serve_https`)
- Task 24685 — `ConversationRuntime` graceful shutdown (potential shared shutdown token)
