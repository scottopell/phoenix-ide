`BashLiveHandleSummary.status: String` carries one value today --
`"running"` -- with a comment that admits "reserved for future
state-aware listings." AGENTS.md correct-by-construction: a field that
is always one value is either dead state (delete it) or the start of an
enum (type it).

## Verified location

`crates/phoenix-ide/src/api/wire.rs:887-902`

```rust
/// One entry of the live-handle snapshot returned with `handle_cap_reached`
/// (REQ-BASH-005).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashLiveHandleSummary {
    pub handle: String,
    pub cmd: String,
    /// Optional handle label set on the run call (REQ-BASH-002). Echoed
    /// here so the agent has something stable to identify the handle by
    /// even when many concurrent commands share similar `cmd` prefixes.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    pub age_seconds: u64,
    /// Always `"running"` today; reserved for future state-aware listings.
    pub status: String,
}
```

Populated at `crates/phoenix-ide/src/tools/bash/operations.rs:164`:

```rust
status: "running".to_string(),
```

## Why this matters

The TS side sees `status: string` and cannot exhaustively match. Today
that doesn't bite because the value is constant. The moment a second
status appears ("ended", "killed", "exited"), the TS consumer has to
guess at the new tag with no compile-time guarantee that all cases are
handled, and a string typo on the Rust side
(`"runnning".to_string()`) silently ships to the wire.

This is the same family as **task 13020** (bash-signal-stringly-typed,
done) -- one stringly-typed boundary field promoted to a typed enum.

## Fix direction

Two viable paths:

A. Promote to an enum now (preferred if the "future state-aware
   listings" prose is real):

   ```rust
   #[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
   #[ts(export, export_to = "../../../ui/src/generated/")]
   #[serde(rename_all = "snake_case")]
   pub enum BashLiveHandleStatus {
       Running,
       // future: Ended, Killed, ...
   }
   ```

B. Delete the field. If the value is always `"running"` because this
   snapshot is the *live-handle* list (REQ-BASH-005), the field
   carries no information. The struct's existence in the snapshot
   already implies `running`.

Path B is cleaner if the spec confirms it; path A is the safer
non-breaking step if future variants are real.

## Related
- 13020 (bash-signal-stringly-typed, done -- same family)
- 13029 (bash-toolinput-tagged-enum, ready -- adjacent correctness
  refactor in the bash module)
