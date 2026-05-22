`BashToolInput` is a flat struct -- an `op: BashOp` discriminator plus seven
sibling `Option` fields -- so shapes the domain forbids (op=peek with `cmd`
set, op=run with `signal` set, op=kill with no `handle`) are all structurally
representable at the tool-input boundary. `parse_request()` is a hand-written
runtime layer that re-derives per-op constraints the type could enforce.

## Verified locations
- crates/phoenix-ide/src/tools/bash/types.rs:32-55 -- `struct BashToolInput {
  op: BashOp, cmd: Option<String>, handle: Option<String>, label, wait_seconds,
  signal, lines, since }` -- every payload field `Option`.
- crates/phoenix-ide/src/tools/bash/operations.rs:88-108 -- `enum BashRequest`
  (Run/Peek/Wait/Kill), each carrying exactly its required fields. Its own doc
  comment (operations.rs:83-86) states the correct-by-construction goal:
  "there is no shape representable that has both `cmd` and `peek`."
- crates/phoenix-ide/src/tools/bash/operations.rs:250-323 -- `parse_request()`
  and `resolve_handle()`: `ok_or_else(...MutuallyExclusiveModes...)` runtime
  guards at lines 254, 267, 317 enforce "op=run requires cmd" and
  "op=peek/wait/kill requires handle" -- exactly the constraints a
  serde-tagged enum makes structural.

## Borderline -- read before acting (filed p3, not p2)
`BashToolInput` is a system-boundary input type and `parse_request()` is a
legitimate parse-don't-validate step; a loose boundary type parsed into the
strict `BashRequest` is an acceptable pattern, not a bug. Tasks 02709 and
13020 both recently reworked this struct and deliberately kept the flat shape.
This is filed as a refinement.

## The improvement
serde supports internally-tagged enums
(`#[serde(tag = "op", rename_all = "snake_case")]`). `BashToolInput` could BE a
tagged enum with Run/Peek/Wait/Kill variants carrying only their valid fields,
producing byte-identical JSON wire form while making the illegal field
combinations unrepresentable and eliminating the cmd/handle-presence runtime
guards. The default-fill tolerances in `parse_read_args` (since=0, lines+since
collision) are unrelated and stay as-is.

## Caveats to evaluate during implementation
- ts-rs codegen output for a tagged enum vs the flat struct -- confirm
  ui/src/generated/ consumers still typecheck.
- The bash tool's JSON `input_schema()` shown to the LLM -- confirm it is
  hand-written (not derived from `BashToolInput`) or update it accordingly.
- `#[serde(deny_unknown_fields)]` interaction with `#[serde(tag)]`.

## Related
- 02709 (bash-tool-agent-ux-pit-of-success, done -- retired legacy
  affordances, kept the flat shape)
- 13020 (bash-signal-stringly-typed, done -- typed one field of this struct)
- 13024 (tooloutput-success-bool-regresses-toolou -- same
  correct-by-construction family)
