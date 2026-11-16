# Preserve the forced-cancellation notice through cooperative child drain

REQ-BED-005a requires one user-visible notice after forced cancellation completes. In `ConversationRuntime::handle_cancelling_tool_timeout`, the retained task is aborted and ToolAborted is applied, but the notice is emitted only if that immediately reaches Idle. If children remain, the parent enters CancellingSubAgents. A later cooperative final-child result reaches Idle without remembering or publishing the owed forced-tool notice. The separate child deadline emits a notice only when it fires.

Implementation base: `d3b57573ccf8662d05125338aad2307bb03da598`. Trace `handle_cancelling_tool_timeout` -> `process_event(ToolAborted)` -> CancellingSubAgents -> delayed cooperative final `SubAgentResult`; the forced-tool retirement fact is lost before the last child's accepted settlement.

Acceptance: force a tool cancellation while admitted children remain, let those children drain cooperatively before their own deadline, and persist/publish exactly one ordered completion notice after settlement. Preserve immediate completion, inline steering, and child-deadline paths without duplicates. Ordinary cooperative cancellation owes no degraded notice. Failed settlement must not publish completed cancellation. Add deterministic regression coverage and real SQLite ordering evidence; do not weaken cancellation requirements or replay tools.

Scope qualification: the normative forced-retirement fact describes the current live cancellation chain. Existing generic restart/error paths may interrupt or replace that chain; they do not prove cancellation completed. Interrupted/restart notice handling is unqualified and not implemented. This task does not authorize a conversation-wide notice store or new automatic recovery guarantee; classify any proposed recovery behavior under the compatibility requirements before implementing it.

PR #1031 already delivered required settlement ownership, exact child-backstop acceptance receipts, classified definite/unclassified terminalization failures, and reuse of authentic terminal payloads racing delivery. Preserve those authorities and regressions. The remaining scope is the live forced-tool notice across a later cooperative drain, including overlap with the existing child deadline. Cancellation responsiveness and admission-handoff work remain in tasks 75010 and 75011.

## Implementation and local qualification

`ToolCancellationRetired` preserves forced provenance separately from cooperative abort. The phase-scoped `ForcedToolRetirement` marker survives nonfinal child acceptance. Final settlement emits one `CancellationCompleted` intent; the existing terminal-transition retry carries it through a known failed checkpoint. Required settlement receipts admit publication only after the exact parent/child commits and successful enclosing inline work. The child deadline shares that one notice disposition. The authority owner survives the notice write; write failure or unexpected owner loss closes the existing fence.

Pre-feature state rows had no recorded retirement marker. A true-absence `NotObserved` default permits startup reconciliation to decode an obligated parent before its phase is reset. Serialization always writes the marker. This neither reconstructs historical forced cancellation nor qualifies restart notice delivery.

- Behavioral RED: `forced_tool_notice_survives_delayed_cooperative_child_drain` observed zero notices after both accepted children, expected one. Regression checkpoint: `79776a877126efb97dc233c79b5aaefbdb992453`; `/tmp/75009-red-confirmed-01.log`. A separate initial test-authoring compile error and a missing-migrations SQLite fixture error are not behavioral regression evidence.
- Focused GREEN: 17 tests passed in `/tmp/75009-focused-02.log`, including delayed/nonfinal drain, real SQLite acceptance rejection followed by successful retry, exact ordered persisted/published summary and notice after reopen, ordinary cooperative cancellation, immediate and delayed inline steering, existing checkpoint retry, child-deadline overlap/duplicates, failed notice write, owner loss, and preserved PR #1031 boundaries.
- Owning all-target compilation passed in `/tmp/75009-compile-02.log`; formatting, task validation, spec shape and spec anchors passed in `/tmp/75009-static-01.log`.

Independent review, complete owner/boundary qualification and broader repository checks remain pending. The task remains in progress; no merge or deployment is claimed.
