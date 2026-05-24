# Cache `WorkScope` on `ConversationHandle` for lifecycle bridge fan-out

## What

`start_browser_lifecycle_bridge` in `crates/phoenix-ide/src/runtime.rs`
currently does one `get_conversation` DB read per live runtime handle
on every `BrowserSessionLifecycleEvent`. Logic:

```
for conv_id in live_runtimes:
    conv = db.get_conversation(conv_id)      // <-- DB hit
    scope = WorkScope::resolve(conv.id, conv.conv_mode.worktree_path)
    if scope != event.work_scope: continue
    runtime.broadcast_tx.send(BrowserSessionState)
```

Copilot review on PR #139 flagged this as O(N) DB hits per event where
the runtime already had the conversation data available at handle
creation time.

## Why p3

Lifecycle events are rare. Sessions create/kill maybe a few times per
hour per active worktree; idle cleanup fires every minute only when
sessions exist. N is bounded by live runtime handles (typically <10
in practice). `get_conversation` is a single-row indexed read. Real
absolute load is negligible.

But the structural fix is small and the perf headroom is essentially
free, so worth doing eventually.

## Fix

Add a `work_scope: WorkScope` field to `ConversationHandle` (in
`crates/phoenix-ide/src/runtime.rs`). Populate at every
`get_or_create_runtime` callsite (currently three: regular get-or-create,
the upgrade-model re-create, and the spawn callsite).

The bridge then reads `handle.work_scope` directly instead of doing a
DB lookup:

```
let candidates: Vec<(String, SseBroadcaster)> = {
    let runtimes = manager.runtimes.read().await;
    runtimes.iter()
        .filter(|(_, h)| h.work_scope == event.work_scope)
        .map(|(id, h)| (id.clone(), h.broadcast_tx.clone()))
        .collect()
};
for (conv_id, broadcaster) in candidates { broadcaster.send_seq(...); }
```

## Validation

- `./dev.py check` clean
- Manually verify that creating a Worktree-scoped session in a
  continuation chain triggers `BrowserSessionState` on every
  continuation member's SSE stream

## Out of scope

- Removing the DB-derived `WorkScope` resolution elsewhere — only the
  bridge needs the cache. Other call sites (handlers, browser_view)
  have direct access to the `Conversation` row anyway.

## Context

Surfaced during Copilot review of PR #139 (browser-on-WorkScope).
Acceptable to ship #139 with the per-event DB queries; this task closes
the perf gap once #139 lands.
