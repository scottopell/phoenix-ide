---
created: 2026-05-01
priority: p1
status: done
artifact: src/tools/bash/registry.rs
---

Lay the foundation for the bash-tool handle model: types, registry, ring buffer, ToolContext extension, reaper, and the lock-ordering helper that all subsequent bash work depends on.

## In scope

- New module subtree `src/tools/bash/`:
  - `handle.rs` — `Handle`, `HandleState` (`Live` / `Tombstoned`), `LiveData`, `Tombstone`, `FinalCause` (`Exited` / `Killed` / `KillPendingKernel`), `KillSignal`. Includes the `signal_number: Option<i32>` field on the `killed` state.
  - `ring.rs` — `RingBuffer`, `RingLine`, byte-bounded eviction, monotonic per-line offset assignment, `truncated_before` computation.
  - `registry.rs` — `BashHandleRegistry`, `ConversationHandles`, per-conversation cap enforcement, sequential handle-id allocation.
  - `reaper.rs` — `install_reaper()` (`PR_SET_CHILD_SUBREAPER` on Linux, log-and-degrade elsewhere) wired into Phoenix startup; `shutdown_kill_tree()` wired into the existing shutdown handler.
- `ToolContext::bash_handles()` accessor returning `Result<Arc<RwLock<ConversationHandles>>, BashHandleError>` matching the existing `ctx.browser()` shape.
- The single `transition_to_terminal` helper through which all `HandleState` writes pass: holds the `RwLock<Arc<HandleState>>` for write, refuses to regress from a terminal state back to `kill_pending_kernel` (the late-exit / timer race fix).
- `tokio::sync::watch::channel<Option<ExitState>>` setup on `Handle`; `ExitWatchPanicGuard` that publishes `ExitState::WaiterPanicked` on drop so wait callers don't hang on a panicked waiter task.

## Out of scope

- The `BashTool` dispatch and the four operations (Task 2). This task is foundation only — types and infrastructure that Task 2 consumes.
- Wire types, UI, codegen (Task 5).

## Specs to read first

- `specs/bash/requirements.md`: REQ-BASH-004 (ring buffer), REQ-BASH-005 (cap), REQ-BASH-006 (tombstones), REQ-BASH-007 (reaper), REQ-BASH-014 (ToolContext).
- `specs/bash/design.md` sections "ToolContext Extension", "Child Process Reaper", "In-Memory Handle Registry", "Waiter task" (specifically the `transition_to_terminal` helper and the `ExitWatchPanicGuard` panic-guard pattern).
- `specs/bash/bash.allium`: the `Handle` entity definition, the `HandleStatus` / `FinalCause` enums, and the lock-ordering @guidance on `HandleProcessExited` / `HandleKillPendingKernel`.

## Dependencies

None (this is the first task).

## Done when

- `cargo check` passes; `./dev.py check` passes.
- Unit tests cover: ring-buffer line-splitting + eviction + offset monotonicity + `truncated_before`; `transition_to_terminal` refuses to regress from terminal; panic guard publishes the `WaiterPanicked` sentinel; cap enforcement at the registry layer.
- The reaper is wired into Phoenix startup (the `prctl` runs once) and the kill-tree is wired into the shutdown handler (calls reach `shutdown_kill_tree` before the runtime exits).
