Cancelling a conversation MUST return it to Idle within a bounded time, regardless of what any tool is doing. Today it does not: a tool that blocks without polling its cancellation token wedges the conversation in `CancellingTool` indefinitely, and the underlying OS work keeps running. This is a correct-by-construction gap — `CancellingTool` is a state with no guaranteed exit.

## Incident (2026-06-17, prod)
An Explore sub-agent was spawned with cwd `/`. Its `keyword_search` call shelled out to `rg` rooted at `/`, scanning the entire filesystem. The user cancelled. The cancel propagated correctly through the state machine — `AwaitingSubAgents → CancellingSubAgents`, sub-agent `ToolExecuting → CancellingTool`, `Aborting tool execution` logged — but the conversation never reached Idle. The `rg` ran at ~257% CPU for 13+ minutes and was only stopped by manually killing the OS process. No `Tool cancelled` event was ever emitted because `execute().await` never returned.

## Root cause
`Effect::AbortTool` (`runtime/executor.rs`) only calls `token.cancel()` on a cooperative `CancellationToken`. The post-await check at the tool-task spawn site reads `is_cancelled()` *after* `execute().await` returns. Unlike the LLM path — which retains `llm_task_handle` and calls `handle.abort()` — no `JoinHandle` is kept for the tool task, so a tool that never observes the token can never be interrupted, and `CancellingTool` has no terminal exit. Compounding it, `keyword_search` runs `tokio::process::Command::output().await` with no `kill_on_drop` and no race against the token, so even aborting the task would orphan the child `rg`.

## Required behavior
- Cancelling from any non-terminal state reaches `Idle` (or the appropriate terminal / parent-notified state) within a bounded deadline, even if a tool task never returns.
- Tool cancellation is non-cooperative: the executor retains the tool task's `JoinHandle` and aborts it on `AbortTool`, mirroring the LLM path.
- A bounded grace deadline in `CancellingTool` guarantees a terminal transition (synthesize `ToolAborted`) even if neither the cooperative token nor the task abort yields an outcome. `CancellingSubAgents` gets the same guarantee.
- Tools that spawn child processes reap them on cancel — `kill_on_drop(true)` and/or process-group kill — so no OS work outlives the cancel. `keyword_search` specifically must not leave an `rg` running.
- Deliberate exception: `bash.rs` does not proactively kill its child on cancel (durable-handle model — "that's what kill is for"). That policy stays for bash; fire-and-forget tools must reap.

## Spec + tests
- Update the executor cancellation lifecycle spec (Allium/spEARS) with the invariant: `CancellingTool` and `CancellingSubAgents` always reach a terminal state within a bounded deadline.
- Add a regression test: a tool that ignores its cancel token and blocks forever still results in the conversation reaching Idle after cancel, within the deadline.
- Add a test that a child-spawning tool leaves no live child process after cancel.

## Out of scope
The `/`-as-cwd footgun that triggered this incident is tracked separately (cwd floor task). This task is the cancellation guarantee, which must hold for *any* runaway tool — not just the `/` case.
