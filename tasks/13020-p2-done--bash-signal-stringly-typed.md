BashToolInput.signal is Option<String> at the tool-input boundary even though a typed KillSignal enum modelling exactly the two legal values lives ~200 lines away in the same module — a correct-by-construction violation the codebase contradicts itself on.

## Verified locations
- crates/phoenix-ide/src/tools/bash/types.rs:46-47 — `#[ts(optional, type = "'TERM' | 'KILL'")] pub signal: Option<String>` on `BashToolInput`. The `ts(type=...)` annotation itself proves the value set is closed.
- crates/phoenix-ide/src/tools/bash/handle.rs:66-85 — `pub enum KillSignal { Term, Kill }` with `as_str()` ("TERM"/"KILL") and `as_libc()` (SIGTERM/SIGKILL). The correct typed model already exists.
- crates/phoenix-ide/src/tools/bash/operations.rs:302-316 — runtime string match re-validates the closed set: `None | Some("TERM") => KillSignal::Term, Some("KILL") => KillSignal::Kill, Some(other) => <build error "signal=... not recognized">`. The `Some(other)` arm is only reachable because the field is String.

## Why egregious
The domain is two values, the correct enum (`KillSignal`) is used everywhere internally (handle.rs, and KillSignal::Term throughout operations.rs/tests) — the ONLY place it degrades to String is the one input struct. The magic-string sentinels "TERM"/"KILL" are duplicated between handle.rs:75-76 and operations.rs:303-304, so they can drift. An invalid signal is representable and only caught by a hand-written runtime branch. This is the strongest correct-by-construction signal: the codebase demonstrably knows the right pattern in the same module.

## Correct sibling pattern
`KillSignal` enum (tools/bash/handle.rs:66) — and `BashOp` enum (tools/bash/types.rs:9) on the very same `BashToolInput` struct: `op` is already a typed enum deserialized directly by serde. `signal` should follow `op`'s example.

## Fix direction
Make `signal: Option<KillSignal>` (derive Serialize/Deserialize on KillSignal with `#[serde(rename_all = "UPPERCASE")]` or explicit rename so the wire form stays "TERM"/"KILL"). Delete the `Some(other)` runtime arm in operations.rs (now structurally impossible). Regenerate ts-rs codegen (`./dev.py codegen`) and confirm the generated TS still narrows to `'TERM' | 'KILL'`; drop the now-redundant `#[ts(type = ...)]` override if ts-rs derives the union from the enum. Verify the JSON wire form is unchanged with a serde round-trip test. Keep KillSignal as the single source for as_str/as_libc.

## Related tasks
- 13016 (p2 ready), 13018 (p2 ready), 13019 (p3 ready) — sibling correct-by-construction findings from prior audit rounds; this is a new, unfiled instance in the same family.
- 27108 (p1 done) touched bash input *rendering* only; it did not address the field's type.
