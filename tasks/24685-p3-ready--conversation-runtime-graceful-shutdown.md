---
created: 2026-04-14
priority: p3
status: ready
artifact: src/runtime/executor.rs, src/runtime/testing.rs
---

# `ConversationRuntime` has no graceful-shutdown path

## Summary

`ConversationRuntime::run()` loops on its internal `event_rx` until the
channel closes or the state machine reaches a terminal state. The
runtime clones its own `event_tx` internally (for self-emitted events
like `LlmRequestComplete`, `ToolCallComplete`, etc.), so **dropping the
external `event_tx` clone held by a caller is not enough to make the
loop exit** — the internal clone keeps the channel open.

This means there is no way for a caller to say "stop this runtime
cleanly and wait for it to exit." The only options today are:

1. Fire-and-forget via `tokio::spawn(async move { runtime.run().await })`
   and never join the handle (used by `src/runtime.rs::spawn_runtime`).
2. Drive the runtime to a terminal state via events (`UserCancel`,
   `CompleteAllAgents`, etc.) and hope it gets there.
3. Drop the whole tokio runtime the handle was spawned in.

Option 3 is what `#[tokio::test]` does implicitly per test function, so
test leaks are technically benign — the test runtime drops at the end
of the test and aborts all spawned tasks. But it's a workaround, not a
solution.

## Concrete impact

### Tests that want a clean shutdown have to leak

`src/runtime/testing.rs::test_streaming_tokens_ordered_before_message`
spawns a fresh runtime per iteration (`ITERATIONS = 30`) and leaves a
comment acknowledging the leak:

```rust
// Runtime runs until its outcome_rx/event_rx both become empty
// or it hits a terminal state. We don't join it — the runtime
// holds its own internal event_tx clone, so dropping our test
// clone isn't enough to make it exit, and waiting here would
// hang the test. Letting it leak is fine at test scope because
// #[tokio::test] gives each test function its own tokio runtime
// and aborts all spawned tasks when that runtime drops at the
// end of the test — the leaked runtimes never survive into other
// tests.
```

This test passes today, but the leak makes it impossible to assert
things like "after sending X events, the runtime has stopped processing"
— the caller has no way to observe runtime exit.

### Production path also lacks it

`RuntimeManager::spawn_runtime` fires the runtime off with
`tokio::spawn` and never joins. If a conversation needs to be forcibly
torn down (e.g., the project it belongs to is being archived mid-flight,
or the whole app is shutting down), there is no "please stop" signal —
only indirect signals via events. On process shutdown, the tokio runtime
drop eventually kills the task, but any in-flight LLM request or tool
execution gets no chance to clean up.

## Proposed fix

Add a `CancellationToken` to `ConversationRuntime` that `run()`'s select
loop respects. Something like:

```rust
pub struct ConversationRuntime {
    // ...existing fields...
    shutdown: tokio_util::sync::CancellationToken,
}

impl ConversationRuntime {
    pub fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.shutdown.clone()
    }

    pub async fn run(mut self) -> Result<(), RuntimeError> {
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => {
                    tracing::info!(
                        conv_id = %self.context.conversation_id,
                        "runtime received shutdown signal; exiting cleanly"
                    );
                    // Persist any in-flight state, cancel outstanding
                    // LLM/tool work, flush pending SSE events, return.
                    return Ok(());
                }
                Some(event) = self.event_rx.recv() => {
                    // ...existing event handling...
                }
            }
        }
    }
}
```

Callers can then do:

```rust
let shutdown = runtime.shutdown_token();
let handle = tokio::spawn(async move { runtime.run().await });
// ...later...
shutdown.cancel();
handle.await?;  // waits for clean exit
```

Tests get:

```rust
let shutdown = runtime.shutdown_token();
let handle = tokio::spawn(async move { runtime.run().await });
// ... drive the test ...
shutdown.cancel();
let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
```

## Testing

- [ ] Unit test: spawn a runtime with a noop LLM, call `shutdown`,
      assert `handle.await` completes within a short deadline.
- [ ] Unit test: spawn a runtime mid-`RequestLlm`, call `shutdown`,
      assert the LLM task is aborted (not waited on indefinitely).
- [ ] Refactor `test_streaming_tokens_ordered_before_message` to
      `shutdown + handle.await` instead of fire-and-forget, remove the
      leak comment.
- [ ] Integration test: `RuntimeManager::shutdown_all()` cancels every
      active runtime and joins them.

## Related

- Task 24683 — the streaming ordering test that currently leaks 30
  runtimes per run; this is the immediate concrete consumer
- `src/runtime.rs::spawn_runtime` — production-side spawner that also
  lacks a shutdown signal

## Notes

- Surfaced from Copilot review on PR #6. The review claim that "30
  runtimes stay alive until the test process exits" was technically
  inaccurate (per-test tokio runtimes clean up on drop), but the
  underlying design gap it was pointing at is real.
- Not a blocker for the review-session bundle — the runtime leak in the
  regression test is self-contained and the production spawner has
  always worked this way. This is a hygiene task, not a correctness fix.
