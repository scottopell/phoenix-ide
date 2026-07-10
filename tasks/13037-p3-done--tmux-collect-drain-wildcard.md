`tools/tmux.rs:331-336` collapses three distinct failure modes
(timeout, JoinError-cancelled, JoinError-panic) into `Vec::new()` with
a wildcard arm. No `tracing::debug!` records when drained output was
discarded.

## Verified location

`crates/phoenix-ide/src/tools/tmux.rs:326-336`

```rust
/// Bounded join on a drain task. The 2-second timeout protects against
/// pathological pipe-fd-leak scenarios (e.g. a tmux child somehow
/// fork-and-keep that holds the write end open after `kill-server`).
/// Under normal operation the join resolves immediately because the
/// pipe has already EOF'd by the time we reach this call.
async fn collect_drain(task: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    match tokio::time::timeout(Duration::from_secs(2), task).await {
        Ok(Ok(buf)) => buf,
        _ => Vec::new(),
    }
}
```

## Why this matters

AGENTS.md: "When a component drops data because the backend does not
support a feature, this must appear in logs at debug level or above.
Silent omission is indistinguishable from a bug."

The wildcard `_ => Vec::new()` is exactly the smell. Three failure
modes that look identical to the caller:

1. `Err(Elapsed)` -- the 2s timeout fired (the pathological case the
   docstring guards against).
2. `Ok(Err(JoinError::cancelled))` -- the drain task was cancelled.
3. `Ok(Err(JoinError::panic))` -- the drain task panicked.

Case 3 in particular swallows a real bug. Cases 1 and 2 are expected
edge cases but should still be visible at `debug!` for postmortems.

The docstring is itself a soft form of the "comment as spec" smell:
the prose describes when the timeout matters, but nothing in the code
or logs records when it actually fired in production.

## Fix direction

```rust
async fn collect_drain(task: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    match tokio::time::timeout(Duration::from_secs(2), task).await {
        Ok(Ok(buf)) => buf,
        Ok(Err(e)) if e.is_panic() => {
            tracing::warn!(error = %e, "tmux drain task panicked, dropping output");
            Vec::new()
        }
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "tmux drain task cancelled, dropping output");
            Vec::new()
        }
        Err(_) => {
            tracing::debug!("tmux drain task timed out after 2s, dropping output -- pipe-fd-leak guard");
            Vec::new()
        }
    }
}
```

Optionally, distinguish the failure modes in the return type
(`Result<Vec<u8>, DrainError>`) so callers can react -- but that may
be over-engineering for a post-terminal cleanup helper.

## Related
- No prior tasks found.
