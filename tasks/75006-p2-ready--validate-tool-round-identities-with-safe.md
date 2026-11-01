# Validate tool-round identities with safe live and restart failure handling

Round assembly checks cardinality, not identity correspondence. A candidate can contain three results `c,duplicate,duplicate` for assistant calls `a,b,c`. Normal finalized-round persistence already rejects duplicate calls, unknown result IDs, and repeated results before insertion; restart materialization has only the legacy count check. Keep assembly explicitly unvalidated until the owning admission boundary proves identity correspondence.

This pre-existing gap was identified while reviewing the new assembly abstraction: https://github.com/scottopell/phoenix-ide/pull/1002#discussion_r4426726031 . The abstraction was narrowed so it does not claim completion or persistence admissibility. A constructor-only tightening would introduce unsafe failure paths: live reducer callers use expect; startup skips count-invalid materialization and later resets state, while orphan repair cannot retain an assistant that was never persisted.

Implement identity validation together with its owning failure policy. Require unique assistant call IDs and a one-to-one correspondence of result IDs without sorting the supplied results. Valid restart materialization may order calls A,B,C as A,C,B after durable present_svg promotion; preserve that behavior.

Acceptance:
- Reject unknown, missing, duplicate result IDs and duplicate assistant IDs at the appropriate validated boundary, with count-mismatch coverage retained.
- Keep successful output, message identity, metadata, and the caller-supplied valid order unchanged.
- Settle malformed live/provider rounds deterministically without panic, wedged execution, partial transcript writes, or replaying an unknown-effect tool; trace the existing runtime storage-error route before choosing the boundary.
- Define and test startup rejection so buffered assistant/completed results remain durable/recoverable instead of being silently discarded by skip plus reset. Cover ToolExecuting and CancellingTool.
- Add valid A,C,B acceptance, malformed live settlement, and durable restart-retention regressions. Do not treat request-order sorting as identity validation.

Relevant boundaries: phoenix-core domain/tool_round.rs; phoenix-state-machine CheckpointData and completion/cancellation transitions; phoenix-db build_materialized_tool_round, startup reset/orphan repair, and persist_finalized_round_tx; phoenix-ide runtime transition/effect failure handling.

This is separate from the no-visible-impact assembly extraction because rejection and failure settlement must be designed as one coherent behavior change. No production occurrence or executed corruption reproduction is claimed.
