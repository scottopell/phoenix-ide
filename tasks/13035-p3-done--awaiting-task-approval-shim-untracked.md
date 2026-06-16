`AwaitingTaskApproval::task_file` is marked `#[serde(default)]` with a
prose comment calling itself a "rollout shim for pre-1.0 conversations
persisted before this field existed." AGENTS.md requires the shim to
either have a real migration or be tracked as a task. No migration
exists in `db/migrations.rs` for the `state` JSON-TEXT column, and no
task tracks the eventual hard-cutover.

## Verified location

- `crates/phoenix-ide/src/state_machine/state.rs:893-912`

```rust
/// `serde(default)` on `task_file` is a rollout shim for pre-1.0
/// conversations persisted before this field existed. Such rows
/// deserialise with an empty `task_file`, which the executor surfaces
/// as a clear "reject and re-propose" error rather than silently
/// resetting the conversation to `Idle`.
AwaitingTaskApproval {
    #[serde(default)]
    task_file: String,
    title: String,
    priority: crate::task_source::Priority,
    plan: String,
},
```

## Why egregious vs surrounding code

Task **13014** (toolexecuting-assistant-message-serde-de, done) cited
THIS field as the *correct pattern* because it "surfaces a loud error
rather than silent reset" -- but the citation overlooked that the
correctness depends on the shim being temporary AND that the rollout
shim is not actually tracked.

Sibling pattern that does it right: `ToolOutcome::Success::images` /
`ToolContent::images` at `db/schema.rs:543, 552, 767` -- comment cites
**task 13023** explicitly, locked by the
`pre_images_tool_rows_deserialize_to_empty_images` test.

Sibling that does it stricter still: `ToolExecuting` (state.rs:805-809)
deliberately uses strict deserialization because `reset_all_to_idle`
wipes it on startup, with a locking test
(`tool_executing_rejects_rows_missing_fields`). `AwaitingTaskApproval`
cannot use that escape hatch -- it is explicitly EXCLUDED from
`reset_all_to_idle` (`db.rs:1493, 1521`), so an old row genuinely can
reach this code with `task_file = ""`.

## Mitigation in place

The executor surfaces the empty-task_file case as a "reject and
re-propose" error. So the failure mode is loud, not silent. That makes
this lower-impact than 13033/13034 -- the shim works as documented; it
is just not on a deprecation schedule.

## Fix direction

One of:

A. Add a real data migration in `db/migrations.rs` that backfills
   `task_file` for legacy `awaiting_task_approval` rows. Update the
   comment to drop the "rollout shim" framing.
B. Promote the field to non-default with the existing executor error
   path as the explicit policy, document the decision (as 13023 does
   for images), and add a locking test analogous to
   `pre_images_tool_rows_deserialize_to_empty_images`.

## Related
- 13014 (toolexecuting-assistant-message-serde-de, done -- cited this
  field as the correct pattern)
- 13023 (the images shim it should look like)
- 02656 (remove-serde-default-shims, done -- ConvMode equivalent)
