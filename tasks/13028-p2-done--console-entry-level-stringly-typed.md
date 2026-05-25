`ConsoleEntry.level` is a `String` for a closed value set, and the value is
produced by Debug-stringifying an already-typed enum -- a typed value
deliberately downgraded.

## Verified locations
- crates/phoenix-ide/src/tools/browser/session.rs:59-63 --
  `pub struct ConsoleEntry { pub level: String, pub text: String, ... }`
- crates/phoenix-ide/src/tools/browser/session.rs:425 --
  `let level = format!("{:?}", event.r#type).to_lowercase();`
  `event.r#type` is a typed chromiumoxide CDP console-event-type enum; it is
  Debug-formatted into a lowercase String, then stored at session.rs:439-440.

## Why it is a violation (relative to the codebase)
Phoenix models closed value sets as enums everywhere -- `BashOp`,
`KillSignal`, `ConvMode`, `ToolOutcome`, `ChainQaStatus`. `ConsoleEntry.level`
is a lone stringly-typed closed-set field, and uniquely it *starts* from a
typed enum and discards the type. `format!("{:?}", ...)` also couples the
stored string to a `Debug` impl, which is not a stable contract -- any
consumer filtering on `"error"` / `"warning"` matches Debug spelling, not a
checked variant.

## Severity note
Lower stakes than a persisted/co-constrained field -- `ConsoleEntry` is
in-memory browser-capture state (not written to SQLite, not co-constrained).
A genuine but minor missed instance of the codebase's own pattern, surfaced by
a wide-net correctness audit on an otherwise well-policed codebase.

## Correct sibling pattern
`KillSignal` (tools/bash/handle.rs) and `BashOp` (tools/bash/types.rs) --
small closed enums with explicit string mappings.

## Fix direction
Introduce a `ConsoleLevel` enum (log, debug, info, warning, error, plus a
catch-all `Other(String)` or explicit variants for rarer CDP types --
dir/table/trace/assert/etc.). Convert at session.rs:425 via a total `match` on
`event.r#type` instead of `format!("{:?}", ...)`. Keep a `Display`/`as_str`
for the existing `tracing::debug!(level = %level, ...)` call at session.rs:434.

## Related
- 13015 (notifyclient-stringly-typed-effect)
- 13016 (task-source-priority-stringly-typed)
- 13020 (bash-signal-stringly-typed)
Prior correct-by-construction fixes in the same family.
