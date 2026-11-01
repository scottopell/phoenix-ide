# Preserve trusted built-in skill receipts during restart materialization

The live tool-round path uses `tool_result_message_content` / `FinalizedToolResult` and persists a receipt for `ToolOutcome::TrustedInstructions`. Startup's `build_materialized_tool_round` instead maps completed results with `result.output()`, writing the authenticated instruction body into the transcript.

Source-grounded reachable shape: a Global Coordinator built-in skill completes before a later call in one assistant batch. The reducer stores its `TrustedInstructions` in `ToolExecuting.completed_results` and persists that state. Restart while a later tool runs (or cancellation remains in progress) normalizes the completed result, and materialization writes its raw output. The startup selector does not universally exclude Coordinator conversations. No production occurrence is claimed; an executed regression reproduction remains required.

Authority: `specs/builtin-skills/builtin-skills.allium`, rule `DeliverCoordinatorBuiltinToModel`, requires the visible and persisted result to be a receipt while the trusted payload remains opaque.

Relevant boundaries: `crates/phoenix-tools/src/skill.rs`, `crates/phoenix-core/src/domain/db_schema.rs` (`tool_result_message_content`), `crates/phoenix-state-machine/src/transition.rs` (completed-results persistence), and `crates/phoenix-db/src/lib.rs` (`normalize_in_flight_round`, `build_materialized_tool_round`).

Acceptance:
- Add a restart/materialization regression with a completed trusted-skill result followed by an interrupted tool; persisted and visible content must contain only the receipt.
- Cover both ToolExecuting and CancellingTool recovery without changing other restart output, ordering, images, durations, or spawn fan-in policies.
- Keep live trusted payload delivery and its audience checks intact; do not reconstruct privileged live instructions from transcript receipt text.

Discovered during the tool-round completion extraction. Address separately so the refactor does not conceal a change in persisted content behavior.
