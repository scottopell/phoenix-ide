# Stop reintroduced tmux auto-wakes from blocking conversation cleanup

## Observed journey

On production conversation `repro-bug-with-mock-server`, selecting **Clean up** calls the legacy `POST /api/conversations/:id/mark-merged` endpoint and returns `Conversation has pending background work` (`error_type: pending_wake`). The conversation itself is durably Idle and has no continuation.

Production is running stale commit `57bbe134a4dc`, but current `origin/main` at investigation time (`c0e55db1f`) still contains the producer defect, so a deploy would only clear the current rows at startup and would not prevent recurrence.

## Verified findings

- Read-only production DB inspection resolved the slug to conversation `98738698-1f67-4039-8fde-84726af4ac08`, state `Idle`, unarchived, with no `continued_in_conv_id`.
- The conversation owns exactly two tmux wake workflows (`390`, `410`). Both workflows and bindings are already cancelled/resolved with typed terminal receipts, but each canonical reducer delivery remains `Pending` with no message link.
- Those are the only pending wake deliveries in the production database and this is the only affected conversation. Other cancelled wake deliveries are `Suppressed`.
- `WakeRepository::has_owed_work_for_conversation` therefore truthfully blocks `mark_merged`; weakening the lifecycle guard would hide the phantom producer rather than fix it.
- `AGENT_FACING_WAKE_REGISTRATION` is false and the wake worker is deliberately not started. Startup correctly calls `retire_all_registrations`, which suppresses persisted old-path obligations.
- Despite that dormant boundary, `tmux_run` automatically calls `register_tmux_wake_if_live` for ordinary live/readiness responses. With no worker, cancellation can leave terminal deliveries pending indefinitely.
- Completed task `44008` and `specs/wake-contracts/executive.md` require the opposite: ordinary bash/tmux handle creation must not create wake obligations while the explicit `wait_until` surface is unimplemented.
- Git history identifies regression commit `104e02518` (`Replace derived resource scopes with durable WorkScope IDs`) as reintroducing the tmux registration helpers and tests after task 44008 removed automatic wakes. Current tests explicitly expect `wake_registration` from ordinary tmux runs, so they encode the regression.

## Interaction map

`tmux_run` ordinary live response → injected `ProductionWakeRegistrar` → durable wake binding/workflow → cancellation creates terminal receipt plus Pending reducer delivery → dormant wake worker never materializes/suppresses delivery → `has_owed_work_for_conversation` → legacy Clean up / mark-merged returns `pending_wake`.

Startup retirement is the recovery edge for persisted old-path rows. The future explicit wait surface/task `44009` is the only intended producer boundary and remains separate.

## Proposed scope

Restore the dormant-wake invariant at the narrow producer boundary without overlapping ProductConversation Close PR #633:

1. Make `tmux_run` incapable of producing wake registrations: remove `register_tmux_wake_if_live`, its wake-only fingerprint/identity plumbing, and `wake_registration` output enrichment from every ordinary live/readiness response.
2. Preserve `REQ-TMUX-014` window semantics inside `crates/phoenix-tools/src/tmux/run.rs`. Default/`keep_open_on_exit = true` windows remain inspectable. `keep_open_on_exit = false` return-immediately windows close naturally at command exit. Readiness-wait windows remain observable until the final readiness/exit result, then restore tmux's own exit cleanup so the window closes when the command completes without a wake worker.
3. Replace the reintroduced registration-positive tmux tests with regressions proving return-immediately, readiness-success, readiness-timeout, and close-after-completion paths never call a registrar or emit a wake acknowledgment while preserving window cleanup behavior.
4. Preserve the durable wake substrate and existing startup `retire_all_registrations` recovery unchanged. Run its focused regression to prove persisted old-path rows become non-owed with audit history retained.
5. Do not edit `RuntimeManager`, wake repository/lifecycle APIs, `has_owed_work_for_conversation`, or Close orchestration in this patch. Those files overlap critical-path PR #633; shared capability hardening or Close-driven wake settlement belongs there or in a separately coordinated follow-up after it lands.
6. Update current-reality/spec text only if the code-only restoration exposes actual drift; run the authoring preflight for any spec edit.

## Acceptance evidence

- Ordinary tmux return-immediately and readiness-timeout/live-window paths preserve their window/result semantics but create no wake workflow and expose no `wake_registration` field.
- `keep_open_on_exit = false` retains deterministic command-exit cleanup without relying on a wake registration or running wake worker.
- Persisted regression rows are canonically suppressed on restart with workflow/message audit records retained.
- The affected production-shaped lifecycle journey no longer returns `pending_wake` after startup reconciliation.
- Focused tmux and existing wake-retirement tests pass; `./dev.py check` passes.

## Risks and non-goals

- Do not implement the explicit `wait_until`/park-on-wake feature owned by task 44009.
- Do not bypass, narrow, or special-case `has_owed_work_for_conversation` for legacy Clean up.
- Do not start the dormant wake worker as a workaround; that could reintroduce duplicate semantic terminal delivery and surprise LLM turns documented by task 44008.
- Do not delete wake audit history or mutate production data manually. A normal fixed deployment restart should perform canonical retirement.
- Do not change shared runtime/wake/lifecycle wiring while ProductConversation Close PR #633 is validating exact head.
- Unified Close/History lifecycle work remains owned by task 92012; this task fixes the current shipped legacy cleanup blocker without absorbing that larger migration.

## Implementation result

- Removed all ordinary `tmux_run` wake registration, wake identity/fingerprint construction, and response enrichment from the tmux runner.
- Restored tmux-native readiness preservation for `keep_open_on_exit = false`: the pane remains observable through readiness/exit capture, then `remain-on-exit` is atomically cleared (or an already-dead window is killed) so command completion owns cleanup.
- Kept default inspectable windows and return-immediately close-on-exit behavior unchanged.
- Replaced wake-positive regressions with no-call/no-payload coverage for return-immediately, readiness success, readiness timeout, and close-after-completion paths.
- Left startup retirement and every runtime, wake-repository, lifecycle, and Close surface unchanged.

## Verification

- Red test before implementation: `return_immediately_does_not_register_wake` failed because the response contained `wake_registration`.
- Focused tmux runner suite: 15 passed.
- Full `phoenix-tools` suite: 529 passed, 1 intentionally ignored; doc tests 2 intentionally ignored.
- Existing startup-retirement regression `retire_all_registrations_clears_owed_work_and_preserves_audit`: passed and asserts the retired conversation has no owed wake work.
- `./dev.py tasks validate`: 948 task files passed.
- `./dev.py check`: all 18 applicable checks passed, including clippy, e2e, Rust tests, codegen-stale, and the Linux musl compile.
