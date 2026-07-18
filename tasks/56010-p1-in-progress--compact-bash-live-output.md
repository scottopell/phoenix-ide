# Make compact bash rendering informative and live

## Problem

Compact density currently reduces completed bash calls to cards such as `bash / exited 0 / done`. The command is available on the `tool_use` block and final duration is available on the paired tool-result message, but the generic compact strip chooses the result summary instead of retaining both. This makes repeated shell work effectively indistinguishable.

There is also a deeper transport gap: bash already captures stdout/stderr incrementally in a bounded per-handle ring buffer, but conversation SSE carries only the tool start timestamp and final tool result. The process inspector can poll live output after it knows a handle, while the inline conversation tool card cannot receive output from a synchronous `bash run` during its wait window. Compact mode must not be a lower-information mode, and the absence of live inline output should not be hidden as a presentation-only issue.

## User-visible outcome

A compact bash card should remain dense while answering, at a glance:

- what command or handle operation ran;
- whether it is running, exited, failed, killed, or still running under a handle;
- how long it has run or took;
- what it is producing now, via a small live terminal tail;
- whether output was truncated, with full detail still one click away.

Example completed card:

```text
bash                                      done · 18.4s
$ ./dev.py check
✓ fmt  ✓ clippy  ✓ tests
```

Example in-flight card:

```text
bash                                   running · 12s
$ cargo test state_machine
running 184 tests…
```

The live tail is bounded and informational, not a second durable result. The finalized tool result remains authoritative and replaces/clears ephemeral progress state.

## Plan

### 1. Make compact bash cards preserve existing information

- Refactor `ToolStripItem`/`deriveToolStripItems` so input identity and result/status metadata are separate instead of `resultSummary ?? inputSummary` alternatives.
- Always show the bash command preview (or a clear `peek`, `wait`, or `kill` handle summary), using the existing display/command simplification and safe truncation.
- Parse final bash status, exit code/signal, and `duration_ms` from the typed result/display metadata and render them together.
- Render a bounded final output tail from the structured bash `lines` response, including truncation indication, without duplicating the full expanded response.
- Keep click-to-expand behavior and keyboard/accessibility labels fully informative.

### 2. Add typed, bounded live tool-output transport

- Introduce a typed bash/tool-progress capability at the tool execution boundary, associated structurally with the active `tool_use_id`; do not encode progress in log strings or overload finalized `ToolOutput`.
- Have bash publish ring-buffer progress as offset-aware snapshots/deltas while `run`/`wait` is blocked. Include complete lines, trailing partial text when present, truncation state, and enough lifecycle metadata for status/handle presentation.
- Coalesce/rate-limit producer updates and bound every event by line/byte count so noisy commands cannot flood SSE or the replay ring.
- Add a typed SSE event and generated TypeScript/schema support. Route it through the existing total-order/reconnect machinery.
- Store progress in an ephemeral, bounded client map keyed by tool-use ID. Merge updates by offsets, reject stale/out-of-order data through the normal sequence guard, and clear progress atomically when the final tool result arrives or the turn is abandoned/cancelled.
- Do not persist incremental output into message JSON. The bash ring buffer is authoritative while live and the normal persisted tool result is authoritative after completion, avoiding parallel durable representations.

### 3. Render live progress in compact and full density

- Add a small reusable bash output-tail view shared by compact and full tool presentations.
- In compact density, keep the command/status/duration visible and show only a bounded recent tail (for example, the latest 2–3 visual lines with a strict height).
- In full density, feed the same live progress into the existing bash response presentation so inline output advances before the final result lands.
- Preserve the process inspector as the full live drill-down for handle-backed jobs; do not add a competing polling loop to each conversation card.
- Reconcile `REQ-CONV-022` with the desired behavior: density controls layout, not whether live progress exists. Replace the current “all in-flight turns render full regardless of density” rule if necessary so compact mode can remain compact while still showing live data.

### 4. Specifications and verification

Update the normative specs before or with implementation:

- `specs/conversation-ui/requirements.md` for compact cards retaining command/status/timing and rendering bounded live output;
- `specs/bash/requirements.md` and, if lifecycle precision warrants it, `specs/bash/bash.allium` for the progress publication contract, bounds, terminal handoff, cancellation, and reconnect behavior;
- relevant executive status/coverage rows, following `specs/AUTHORING.md` pre-flight rules.

Add tests for:

- compact `run`, `peek`, `wait`, and `kill` command/handle previews;
- completed success, non-zero exit, signal/killed, still-running handle, duration, and truncated output states;
- live output appearing before `ToolComplete` in both densities;
- partial lines, offset merge, duplicate/out-of-order SSE, bounded tail size, and coalescing;
- final result atomically replacing ephemeral progress;
- reconnect replay during execution and cleanup after cancellation/abandonment;
- typed Rust wire ↔ generated TypeScript ↔ Valibot parity;
- accessibility and click-to-expand behavior.

Add/update the tool-results Ladle fixture with running and completed compact bash examples and capture desktop/mobile QA screenshots. Run focused Rust/UI tests, codegen, spec validation, fixture QA, and `./dev.py check`.

## Acceptance criteria

- A completed compact bash card never collapses to only `exited 0`/`done`; it visibly includes command/operation identity and final duration.
- A synchronous bash command that emits output before returning visibly updates its inline card while it is running.
- Compact and full density receive the same live facts; compact changes spatial presentation only.
- Live updates are typed, bounded, coalesced, reconnect-safe, and removed on terminal handoff.
- No per-card polling loop or duplicate persisted output representation is introduced.
- Expanded bash detail and the process inspector continue to expose the complete available response/ring-buffer detail.
