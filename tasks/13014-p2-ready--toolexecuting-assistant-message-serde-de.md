CoreState::ToolExecuting.assistant_message uses #[serde(default)] as a substitute for a real DB migration, causing silent conversation data loss on restart.

Location: crates/phoenix-ide/src/state.rs:691 (duplicate at state.rs:831)

ConvState persists as a JSON-in-TEXT SQLite column and there is no `state` data migration anywhere in db/migrations.rs. A row persisted mid-ToolExecuting deserializes to an empty AssistantMessage -- silent loss of the load-bearing assistant turn that drives the next LLM round. No migration, no tracking task, no rollout-shim comment.

Contrast: AwaitingTaskApproval.task_file (state.rs:771) is the correct pattern -- documented shim, surfaces a loud error rather than silent reset, tracked by a task. conv_mode also has guarding proptests. This field was simply missed and the defaulted value is silent and semantically load-bearing.

Fix direction: add a real migration backfilling/handling the state column, or make the empty-assistant_message case surface a loud error like task_file does (and document the shim with a tracking reference). Also check PendingSubAgent.mode (state.rs:1530) which has the same hazard -- defaults a running sub-agents mode on restart.
