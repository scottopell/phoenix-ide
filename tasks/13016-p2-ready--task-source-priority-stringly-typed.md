TaskSource::Taskmd flattens an already-typed priority into String while keeping its sibling status typed — a correct-by-construction violation the codebase contradicts one line away.

## Verified locations
- crates/phoenix-ide/src/task_source.rs:33 — `priority: String` field on `TaskSource::Taskmd`
- crates/phoenix-ide/src/task_source.rs:55 — `priority: parsed.priority.to_string()` (taskmd_core returns a typed priority; it is flattened here)
- crates/phoenix-ide/src/task_source.rs:34,56 — sibling `status: Status` is kept as the typed `taskmd_core::constants::Status` enum (the correct pattern, same struct, adjacent line)
- crates/phoenix-ide/src/task_source.rs:79-83 — `priority()` returns `&str`, with the `"p2"` literal fallback structurally indistinguishable from a parsed value
- Untyped String then propagates: state_machine/effect.rs:180, state_machine/state.rs:780 & :877, state_machine/transition.rs:45, runtime/executor.rs:2413, api/types.rs:320

## Why egregious
The domain is closed (`p0`..`p4`, per AGENTS.md and specs/tasks-ui/design.md). taskmd_core::filename::parse_filename already returns a typed priority. The code keeps `status` typed but `.to_string()`-flattens `priority` on the very next line, then threads the bare String through the effect/state/executor/api layers with zero validation — an invalid priority reaches the approval UI silently. The codebase demonstrably knows the right pattern: ToolOutcome (db/schema.rs:529), MessageType (db/schema.rs:955), ChainQaStatus (db/schema.rs:989), ConvMode (db/schema.rs:207), and `status: Status` on this very struct.

## Correct sibling pattern
`status: Status` on `TaskSource::Taskmd` (task_source.rs:34) — keep the typed value taskmd_core hands back instead of stringifying it.

## Fix direction
Introduce a typed Priority (newtype or enum, ideally reuse taskmd_core's type if exported, else a local enum with as_str/from_str round-trip like the db/schema.rs enums). Replace `priority: String` on TaskSource and thread the typed value through Effect::ApproveTask, ParentState::AwaitingTaskApproval, transition.rs, executor.rs. The wire/api boundary (api/types.rs:320) may keep a String representation via the typed value's as_str().

## Related tasks
- 13009 (done) introduced TaskSource but only decoupled task-source kinds; it did not address priority typing.
