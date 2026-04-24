---
created: 2026-04-24
priority: p3
status: ready
artifact: specs/bedrock/bedrock.allium
---

Three rules in `specs/bedrock/bedrock.allium` (~lines 616, 627, 636) subscribe to a trigger named `TaskApprovalDecided` (or `UserApprovesTask` / `UserProvidesFeedback` / `UserRejectsTask` — check the actual spelling), but the runtime emits `TaskApprovalResponse` on the corresponding endpoint. The trigger names in the spec do not match what fires.

Found by the projects.allium audit on 2026-04-24 (sub-agent investigation under task 24696). Out of scope for that PR; filed separately.

## Why this matters

`allium check` is file-local (see task 02683), so cross-spec event-name typos pass clean. This is a same-file shape mismatch — the bedrock surface declares `TaskApprovalDecided(conversation, decision)` (around line 999 of bedrock.allium) but no runtime emitter uses that name. The corresponding Rust handler is `respond_to_task_approval` / `approve_task` in `src/api/lifecycle_handlers.rs` and the response type is `TaskApprovalResponse` in `src/api/types.rs`.

Either:
- The spec is right and the runtime should emit a `TaskApprovalDecided` event explicitly (current code emits `TaskApprovalResponse` as an HTTP response, not an SSE/lifecycle event).
- The runtime is right and the spec should rename the trigger to match what fires.

I lean toward the second — there is no daylight between "user clicks approve in the FE" and "runtime processes approval"; an extra event would be ceremony. But this is a real spec-vs-runtime divergence and either path needs a deliberate decision.

## Why p3

Same severity as 02683 — the rules currently fire on a trigger no real path provides, but the runtime path through `approve_task` / `respond_to_task_approval` handlers does the right thing regardless. Spec hygiene, not user-visible bug.

## Out of scope

The wider phantom-trigger pattern in `projects.allium` (~10 events: `UserSelectsManaged`, `UserSelectsBranch`, `UserSendsFirstMessage`, `PollerFires`, `WriteAttempted`, `SubAgentSpawned`, `TaskApprovalStarted`, `ConversationCreated`, etc.). Those are a different shape — events that ARE emitted by the runtime but are not declared on any imported surface. The right fix is a Phoenix-side surface that declares them, which is its own focused workstream. File separately if it becomes worth doing.

## References

- `specs/bedrock/bedrock.allium` ~lines 616/627/636 (the rules) and ~line 999 (the surface declaration)
- `src/api/lifecycle_handlers.rs` (`respond_to_task_approval`, `approve_task`)
- `src/api/types.rs` (`TaskApprovalResponse`)
- Task 02683 — same class of cross-spec drift, different specimen
- Task 24696 — origin of the audit that surfaced this
