---
created: 2026-04-27
priority: p2
status: in-progress
artifact: pending
---

# tool-execution-elapsed-time

## Plan

# Tool Execution Elapsed Time Display

## Summary

Show elapsed time while a tool is running ("running bash … 4s … 5s") and final duration on completed tool blocks ("✓ bash • 3.2s"). Covers both the live-timer experience and the post-completion record.

## Context

- `StateBar.tsx` already shows tool name (e.g. "running bash") during `tool_executing` phase, but no timer
- `src/runtime/executor.rs` already measures `duration_ms` but only logs it — never sent to UI
- `SseWireEvent::MessageUpdated` carries `display_data` but no `duration_ms` field
- `StreamingBuffer.startedAt` in `atom.ts` shows the existing pattern for client-side timestamps

## What to Do

### 1. Rust: Add `duration_ms` to `MessageUpdated` SSE event

In `src/api/wire.rs`, add `duration_ms: Option<u64>` to the `MessageUpdated` variant (or its payload struct).

In `src/runtime/executor.rs`, pass the measured `duration_ms` into the `notify_tool_complete` / `message_updated` emit path so it's included in the SSE event.

Run `./dev.py codegen` to regenerate `ui/src/generated/` TypeScript types.

Update the valibot schema in `ui/src/sseSchemas.ts` to include `duration_ms?: number`.

Verify the `parity_*` tests in `src/api/sse.rs` still pass (add a case if needed).

### 2. UI: Live elapsed timer in StateBar

In `atom.ts`, add a `toolStartedAt: number | null` field to the atom (or co-locate it with the existing phase tracking). Set it to `Date.now()` when processing `sse_state_change` into `tool_executing`, clear it when leaving that phase.

In `StateBar.tsx`, add a `useEffect` + `setInterval(1000ms)` that ticks while `phase.type === 'tool_executing'`. Compute `elapsed = Math.floor((Date.now() - toolStartedAt) / 1000)`. Update the status text from `"running bash"` → `"running bash … 4s"`.

### 3. UI: Final duration on completed tool blocks

In `atom.ts`, when handling `sse_message_updated`, store the incoming `duration_ms` alongside the tool result (e.g. in a `toolDurations: Map<tool_use_id, number>` in the atom, or thread it through `display_data`).

In `MessageComponents.tsx` (`ToolUseBlock`), render the duration next to the status icon when available: `✓ bash • 3.2s` (format as `Xs` for < 60s, `Xm Ys` for longer).

## Acceptance Criteria

- [ ] Sending a message that triggers a slow tool (e.g. `sleep 5`) shows the StateBar ticking: "running bash … 1s", "… 2s", etc.
- [ ] After the tool completes, the tool block header shows the final duration: `✓ bash • 5.1s`
- [ ] Timer clears cleanly when transitioning out of `tool_executing` (no stale interval)
- [ ] `./dev.py check` passes (clippy, fmt, tests, codegen-stale guard)
- [ ] Duration format: `< 60s` → `"4s"`, `≥ 60s` → `"1m 4s"`


## Progress

