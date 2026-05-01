---
created: 2026-05-01
priority: p1
status: ready
artifact: ui/src/sseSchemas.ts
---

Migrate all the surfaces around the bash + tmux + cascade work: SSE wire types, ts_rs codegen, valibot schemas, UI rendering, mock fixtures, and the subagent schema passthrough. Lands after the backend tasks and ties them to the user-visible frontend.

## In scope

- `src/api/wire.rs`:
  - Replace the existing untyped bash tool result variant with a typed enum: `BashSpawnResponse`, `BashPeekResponse`, `BashWaitResponse`, `BashKillResponse`, `BashErrorResponse`. Each carries the fields from REQ-BASH-002 / REQ-BASH-003 (status, handle, exit_code, signal_number, duration_ms, start_offset, end_offset, truncated_before, lines, deprecation_notice, etc.).
  - Add `TmuxToolResponse` typed variant per REQ-TMUX-012 (status, exit_code, duration_ms, stdout, stderr, truncated).
  - Add `ConversationHardDeleted` SSE wire event variant per REQ-BED-032 step 6.
  - All new types: `#[derive(ts_rs::TS)] #[ts(export, export_to = "../ui/src/generated/")]` per the project's codegen convention.
- Run `./dev.py codegen`; commit the regenerated `ui/src/generated/` files. The codegen-stale guard in `./dev.py check` must pass (clean diff on the generated dir).
- `ui/src/sseSchemas.ts` — valibot schemas for the new wire variants. `satisfies v.GenericSchema<unknown, BashSpawnResponse>` etc. so a Rust-side change surfaces as a tsc error here.
- UI rendering updates:
  - Live `running` and `still_running` bash responses: render with a "running" badge on the tool-call card and a peek button.
  - `tombstoned` bash responses: render the `final_cause` distinction (exited / killed) and the optional `signal_number` for human-readable terminal cause.
  - `kill_pending_kernel` bash responses: render with a distinct visual indicator (the kill is in flight; this is not a normal terminal state).
  - peek / wait / kill display labels per REQ-BASH-015 ("peek b-7", "wait b-7 (up to 30s)", "kill b-7 (TERM)", etc.).
  - tmux response: separate stdout / stderr panels (different from bash's combined output); render `truncated` with a [output truncated] indicator.
  - `deprecation_notice` field surfaces visibly to the user / agent when present (no underscore-prefix; the agent should attend to it).
  - `ConversationHardDeleted` SSE event triggers sidebar refresh in the UI store.
- `src/llm/mock.rs` and any other mock-fixture sites (eval harness, integration test fixtures) — update to produce the new bash response shapes. The old `{ output, exit_code }` fixtures will fail to deserialize against the new wire types.
- `src/tools/subagent.rs` — schema passthrough for the new bash tool. The subagent registers bash with the same schema the parent does; ensure the new oneOf-based schema reaches subagents. Verify the deprecation alias works end-to-end (subagent passing `mode` should still work with `deprecation_notice`).
- `phoenix-client.py` — if it parses bash output, update to consume the new shape; otherwise note the contract change in a comment.

## Out of scope

- Anything backend-only that lands in earlier tasks. This task is the migration / surface layer.
- The deferred items in the spec (TmuxStaleRecoveryNotification, BashHandleStdinSupport, etc.) — these stay deferred.

## Specs to read first

- `specs/bash/design.md` "Migration from Prior Revision" and "Output Capture and Display" sections.
- `specs/tmux-integration/design.md` "Migration" section.
- `AGENTS.md` § "TypeScript codegen for SSE types" — the codegen workflow and the codegen-stale guard.

## Dependencies

- 02694 (Bash operations) — the wire types match what BashTool produces; needs to be settled first.
- 02695 (Tmux + terminal) — same for tmux response shape.
- 02696 (Cascade) — the `ConversationHardDeleted` SSE event variant is added here but the broadcast happens in the cascade orchestrator. Must land after.

## Done when

- `./dev.py check` passes — including the codegen-stale guard (`git diff --exit-code -- ui/src/generated/` is clean after the test run).
- `./dev.py up` works; agent calls bash and tmux end-to-end and the UI renders the new response shapes correctly.
- A bash response with status `still_running` shows the running indicator + peek button; clicking peek issues the right call.
- A `tombstoned` response shows `final_cause` + `signal_number` (when present).
- A subagent invoked with the new bash schema works; an older snapshot using `mode=background` shows a `deprecation_notice` in the response and runs.
- The conversation sidebar refreshes when a hard-delete `ConversationHardDeleted` SSE event arrives.
- All existing mock-fixture-based tests pass.
