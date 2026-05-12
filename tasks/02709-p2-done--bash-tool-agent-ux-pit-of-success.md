# bash-tool-agent-ux-pit-of-success

## Plan

# Bash tool agent UX: pit-of-success polish + tolerance retirement

## Summary

Tighten the bash tool API surface for LLM agents, based on real agent feedback (full review in the conversation thread). Two threads, executed together because they touch the same code paths:

**A. UX polish (4 items):**
1. Rename `op="spawn"` → `op="run"`. "Spawn" carries a `fork(2)` / fire-and-forget prior; "run" matches the actual semantics ("run a command synchronously, optionally yielding a handle if `wait_seconds` elapses"). Same class of fix as the prior `timeout` → `wait_seconds` rename.
2. Add an optional `label` field on `op="run"` so agents can annotate handles. Echoed on every response carrying a handle and on each entry of `live_handles[]` in `handle_cap_reached`.
3. Add `tracing::debug!` to the two remaining silent tolerance paths (`since=0` treated as absent; `lines+since` collision resolution).
4. Add a 4-pattern cookbook block to the tool description so the `wait_seconds=0` "give me a handle now" affordance is discoverable.

**B. Retire unjustified tolerance affordances.** REQ-BASH-010's tolerances mix two distinct concerns. Once you split them:
- *Defenses against in-flight conversation history that won't actually occur* (the LLM sees the current schema each turn and conforms; pre-discriminator history is inert text from the model's POV) — these are dead weight and should be deleted, not maintained.
- *Defenses against active GPT default-fill on the current schema* — these are live constraints and must stay.

Concretely: hard-rename `op="spawn"` → `op="run"` (no alias), and drop the four-sibling legacy inference, the legacy empty-string tolerance, the `mode` parameter handling, and the `command` alias for `cmd`. Keep `since=0`-as-absent and `lines+since` collision resolution.

## Context

Source: agent self-postmortem (in this conversation) plus a cross-read of `specs/bash/{requirements,design,executive}.md`, `specs/bash/bash.allium`, and `src/tools/bash/operations.rs`.

The retirement leans on a clean argument: with `op` as a required schema discriminator, current LLMs conform to the current schema each turn. The four-sibling shape, the `mode` parameter, the `command` alias, and the bare-`cmd`-no-`op` shape are not advertised and not produced by current models. The `peek/wait/kill="..."` keys are not even in the schema. Tolerating them costs code, tests, and prose; deleting them is a structural simplification.

Out of scope (deliberately):
- Auto-escalation TERM→KILL stays disabled per `@guarantee NoAutoEscalation`.
- The bash↔patch tool boundary mentioned in the agent feedback is unrelated to this surface.

## What to do

### 1. Hard rename: `op="spawn"` → `op="run"`

**Wire / schema:**
- Schema `op` enum becomes `["run", "peek", "wait", "kill"]`. `spawn` is gone, no alias.
- `op="spawn"` falls through to schema validation as an unknown enum value — a clean structured rejection (loud failure is the right signal if any caller still emits it).

**Internal Rust:**
- `Op::Spawn` → `Op::Run` throughout `src/tools/bash/`.
- `BashRequest::Spawn { ... }` → `BashRequest::Run { ... }`.
- Helper / function names that embed "spawn" semantics (`run_spawn`, etc.) renamed for consistency.

**Description:**
- The `op="run"` paragraph leads with affirmative semantics: *"Run a shell command. If it finishes within `wait_seconds`, you get its full output and exit code — same as if you'd run it in a shell. If `wait_seconds` elapses first, the process keeps running and you receive a handle to peek/wait/kill later."*
- Add explicit negation against the fire-and-forget prior, mirroring the existing `wait_seconds` negation: *"`run` does NOT detach. The handle is only minted if `wait_seconds` elapses; for short commands you'll just get the result."*

### 2. Retire unjustified tolerance affordances

In `src/tools/bash/operations.rs::RawBashInput`, **delete** the following fields:
- `peek: Option<String>`
- `wait: Option<String>`
- `kill: Option<String>` (legacy op-key form; the `op="kill"` form stays, dispatched via the `op` discriminator and `handle` field)
- `command: Option<String>` (alias for `cmd`)
- `mode: Option<String>`

Delete the corresponding parser logic:
- The four-sibling legacy inference path in `resolve_op` (the "infer from a single non-empty legacy field" code).
- The empty-string-as-absent filtering on those legacy fields.
- The entire `mode` handling block (lines ~317–336): the `(mode, wait_seconds)` match, the deprecation_notice synthesis, the canonical-`wait_seconds` mapping.
- The `command` fallback in the cmd resolution (`raw.cmd.or_else(|| raw.command)`).

Knock-on cleanup:
- `BashRequest::Run` and `BashRequest::Wait` no longer carry `deprecation_notice`. Remove the field from the request enum, response payloads, and all function signatures that thread it through (`run_spawn`/`run_run`, `run_wait`, `still_running_response`, `terminal_or_panic_response`, response builders).
- `deprecation_notice` is removed from response shapes in `src/api/wire.rs` (TS codegen will regenerate).
- The `mutually_exclusive_modes` error path: with `op` required and legacy inference gone, the "no op key / multiple op keys" cases are unreachable. Either delete the error variant entirely or repurpose into a `missing_op` runtime guard if schema validation isn't enforced before tool dispatch (verify during implementation; prefer deletion).

`#[serde(deny_unknown_fields)]` on `RawBashInput` already gives us the right behavior post-cleanup: any caller still emitting `mode`, `command`, `peek`, `wait`, `kill` (as top-level keys) gets a structured parse error.

**Keep** (live concerns on the current schema):
- `since=0` treated as absent — current GPT models default-fill optional integers below their declared `minimum`.
- `lines + since` both provided → prefer `lines`, drop `since` — current GPT default-fill collision on optional integers.

### 3. Add optional `label` field

**Wire / schema:**
- New optional `label: string` schema property, documented as "optional human-readable handle annotation for `op="run"`; echoed back on every response carrying a handle and shown in `handle_cap_reached.live_handles[]`."
- Soft length cap (e.g. 64 chars) enforced; oversize labels rejected with a structured error (`error: "label_too_long"` with the cap).

**Data model:**
- `Handle` struct (`src/tools/bash/handle.rs`): add `label: Option<String>`, set at construction.
- `LiveHandleSummary` (`src/tools/bash/registry.rs`): add `label: Option<String>`, populated from the handle.
- `BashLiveHandleSummary` (`src/api/wire.rs`): add `label: Option<String>` — triggers TS codegen regen.

**Response shapes (`src/api/wire.rs`):**
- Every response variant that carries `handle` adds `label: Option<String>`:
  - `still_running` (run / wait re-timeout)
  - `exited`
  - `tombstoned` (peek / wait / kill)
  - `kill_pending_kernel`
- `live_handles[]` entries on `handle_cap_reached` carry `label`.

### 4. `tracing::debug!` on the two surviving silent paths

In `src/tools/bash/operations.rs`:
- The `since=0`-as-absent path (around line 494): add `tracing::debug!(provided_since = 0, "bash input: dropping since=0 (likely default-fill); use lines= or omit since")`.
- The `lines + since` collision path (around line 503): the existing `tracing::debug!` already covers this — verify wording and field tags are consistent.

### 5. Cookbook in tool description

Prepend a compact 4-pattern block to the description (above the per-op detail):

```
Common patterns:
  - Run synchronously:    op="run", cmd="...", wait_seconds=30
  - Start in background:  op="run", cmd="...", wait_seconds=0  (returns a handle immediately)
  - Inspect progress:     op="peek", handle="b-3"
  - Wait for completion:  op="wait", handle="b-3", wait_seconds=60
```

The detailed per-op paragraphs stay; the cookbook is the agent's first read.

### 6. Spec updates

`specs/bash/requirements.md`:
- REQ-BASH-002: `spawn` → `run` in prose; refresh the negation-framing example.
- REQ-BASH-010: substantial rewrite. The advertised schema is `op ∈ {run, peek, wait, kill}`, `cmd` (required for `op=run`), `handle`, `wait_seconds`, `signal`, `lines`, `since`, `label`. Document the two surviving tolerances (`since=0`-as-absent, `lines+since` collision) under their live-default-fill rationale. Move the four-sibling / `mode` / `command` history into a brief "retired affordances" note in the Rationale section — historical context only, no behavioral rules.
- Tail of REQ-BASH-002 / REQ-BASH-003: a sentence on the `label` echo contract.

`specs/bash/design.md`: schema sketch and operation table reflect the new shape.

`specs/bash/executive.md`: any "spawn" references in the technical summary; mention the retirement and `label` addition.

`specs/bash/bash.allium`:
- `Op` enum: `spawn` → `run`.
- Surface rule `AgentCallsBashSpawn` → `AgentCallsBashRun`; signature gains an optional `label`.
- `Handle.label: String?`; `LiveHandleEntry.label: String?`.

### 7. Tests

Delete (testing now-removed behavior):
- `mode_with_wait_seconds_silently_drops_mode`
- `mode_alone_succeeds_and_includes_deprecation_notice`
- `legacy_cmd_only_shape_still_succeeds`
- `gpt_default_fill_shape_is_tolerated` (relied on empty-string tolerance + `mode` + every legacy field at once)
- `no_operation_keys_returns_mutually_exclusive_modes` and `multiple_operation_keys_returns_mutually_exclusive_modes` if those error paths are deleted; otherwise rewrite for the new minimum-shape error.

Keep (covers active tolerance):
- `peek_with_since_only_routes_to_incremental_mode`
- `peek_with_lines_and_since_drops_since_silently`

Rename / update for the rename:
- `op_spawn_with_cmd_succeeds` → `op_run_with_cmd_succeeds`
- `op_peek_with_handle_routes_through_lookup` and any other tests using `op="spawn"` strings updated.

Add:
- `op_spawn_returns_schema_error` (or the equivalent runtime parse error if validation runs at tool layer): `{"op": "spawn", "cmd": "echo hi"}` is rejected with a structured error.
- `legacy_top_level_mode_returns_schema_error`: `{"op": "run", "cmd": "echo hi", "mode": "default"}` is rejected by `deny_unknown_fields`.
- `legacy_top_level_peek_key_returns_schema_error`: `{"peek": "b-3"}` is rejected.
- `label_round_trips_through_run_peek_wait_tombstone`: spawn with `label="dev-server"`; peek/wait response carries label; tombstone after exit carries label.
- `label_appears_on_cap_reached_live_handles`: cap-reached error includes label per entry.
- `label_over_cap_returns_structured_error`: 65-char label rejected with the cap and a hint.

## Acceptance criteria

- [ ] Schema advertises `op` enum as `["run", "peek", "wait", "kill"]`. `spawn` does not appear.
- [ ] `RawBashInput` no longer has `peek` / `wait` / `kill` / `command` / `mode` fields. `deny_unknown_fields` is preserved.
- [ ] `BashRequest::Run` and `BashRequest::Wait` no longer carry `deprecation_notice`; the field is removed from response shapes; TS codegen reflects the removal.
- [ ] `since=0`-as-absent and `lines+since`-collision tolerances remain, both with `tracing::debug!` lines that name the tolerance and the dropped value.
- [ ] Tool description leads with a 4-pattern cookbook; the `op="run"` paragraph carries explicit negation against the fire-and-forget prior.
- [ ] Optional `label` accepted on `op="run"`; echoed on `still_running`, `exited`, `tombstoned`, `kill_pending_kernel` responses; included on each `live_handles[]` entry on `handle_cap_reached`. Length cap rejection emits a structured error.
- [ ] `specs/bash/requirements.md` (REQ-BASH-002, REQ-BASH-010), `design.md`, `executive.md`, and `bash.allium` updated consistently — `Op::Run`, `AgentCallsBashRun`, `Handle.label`, `LiveHandleEntry.label`. Retired affordances are described as historical context, not as behavioral rules.
- [ ] TS codegen regenerated under `ui/src/generated/`; `git diff --exit-code -- ui/src/generated/` is clean after `./dev.py codegen`.
- [ ] All bash-related tests (`cargo test -p phoenix-ide bash`) pass after the rename, the deletions, and the new additions.
- [ ] `./dev.py check` passes (clippy + fmt + tests + codegen-stale guard + spec validation).
- [ ] After Rust changes, `./dev.py restart` issued and the UI URL handed back to the user.

## Notes / risks

- **In-flight conversation impact.** Conversations whose history contains `op="spawn"` will see those tool calls as historical text only — they're not re-invoked. New tool calls in those conversations conform to the new schema. The risk surface is genuine resumption-mid-tool-call (rare, already fragile for other reasons); we accept this rather than maintain an alias forever.
- **`mutually_exclusive_modes` error code.** With `op` required and legacy inference gone, this code's producer disappears. Verify schema validation runs before tool dispatch (in which case the variant can be deleted); if not, repurpose into a `missing_op` runtime guard. Decide during implementation, prefer deletion.
- **Allium rename.** Search for `AgentCallsBashSpawn` / `Op::spawn` references in any downstream Allium tooling before merging.
- **Wire-format parity.** `src/api/sse.rs` has `parity_*` tests guarding the SSE wire format against typed-vs-`json!()` drift. Verify they still pass after the response-shape edits (label addition, deprecation_notice removal).
- No DB migration needed: handles and tombstones are in-memory only (REQ-BASH-006).

## Progress

