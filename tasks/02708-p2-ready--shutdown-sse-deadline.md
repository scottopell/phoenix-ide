---
created: 2026-05-05
priority: p2
status: ready
artifact: src/main.rs
---

Add a hard shutdown deadline so SSE streams cannot pin the binary alive
forever after a deploy.

## Symptom

After `./dev.py prod stop && ./dev.py prod deploy`, the old phoenix process
can stay alive indefinitely (observed: 23h+ of ELAPSED) holding accepted SSE
streams open. The new binary takes over the listen socket fine — Linux
allows that — but the orphaned old process keeps its accepted connections
in ESTABLISHED until the client decides to disconnect, which for an SSE
stream with keepalive pings is "never."

`./dev.py prod stop` papers over its own daemon (it has its own timeout
and SIGKILLs after a grace period), but unrelated zombies remain. And the
stuck-shutdown behavior is still wrong inside the running process: an SSE
client will hold a deploy hostage until SIGKILL.

## Root cause

`src/main.rs:275` runs the server with `with_graceful_shutdown(...)` and
no outer deadline. axum's graceful path waits for every in-flight request
to complete; SSE streams complete when the client disconnects, which they
won't.

## Fix

Wrap the `with_graceful_shutdown` await in a `tokio::time::timeout` (e.g.
5–10 seconds). On expiry, drop the server future so all remaining
connections are torn down. Existing bash-handle SIGKILL pass on line 283
already follows the same "bounded final cleanup" pattern — mirror it.

## Done When

- [ ] Graceful shutdown bounded to N seconds (configurable constant)
- [ ] After timeout, remaining connections are forced closed
- [ ] Manual repro: open an SSE stream, send SIGTERM, process exits within deadline
- [ ] No regression to legitimate fast shutdowns (no API requests in flight)

## Notes

Discovered 2026-05-05 during a prod incident where the user's browser
showed "pending forever" on an Approve Task POST. Investigation found a
23h-old zombie phoenix process holding fd=15 on a stuck SSE stream from
a different remote client — graceful shutdown had been waiting on that
stream since the prior deploy. Companion task: bounded SSE stream
lifetime (self-healing on the stream side too).
