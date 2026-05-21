ToolContent.images and ToolOutcome::{Success,Error}.images use #[serde(default)] to absorb schema drift on old persisted rows in the messages.content JSON-in-TEXT SQLite column, with NO migration and NO tracking task.

Verified locations:
- crates/phoenix-ide/src/db/schema.rs:534-535 and :541-542 -- ToolOutcome::Success.images / ToolOutcome::Error.images: `#[serde(default, skip_serializing_if = "Vec::is_empty")] images: Vec<ToolContentImage>` with NO comment, NO task, NO migration. Textbook bare serde(default).
- crates/phoenix-ide/src/db/schema.rs:751 -- ToolContent.images: same shim, but the doc comment ("`#[serde(default)]` ensures old DB rows (no `images` key) deserialize to empty vec") explicitly states it is absorbing row drift while there is no migration and no task -- a comment that falsely implies the shim is sanctioned.

Why egregious: these structs serialize into the messages.content JSON-TEXT column; there is no `state`/content data-backfill migration for them anywhere. AGENTS.md: "serde(default) on a JSON-in-TEXT column field is a patch around a missing migration ... the migration must exist or be tracked as a task."

Correct sibling pattern (codebase knows the right way):
- schema.rs ConvMode::Explore{worktree_path} -> data-backfill MIGRATION_007, tracked by task 03001.
- continued_in_conv_id shim references task 24696 (migration 003); chain_name references task 02686 (migration 005).
- These images fields were simply missed.

Related tasks:
- 13014-p2-ready--toolexecuting-assistant-message-serde-de: same class (serde(default) on JSON-TEXT state column, no migration) for CoreState::ToolExecuting.assistant_message -- already tracked separately; this task covers the messages.content images fields, not state.
- 02656-p1-done--remove-serde-default-shims: removed ConvMode shims only; did not touch images.

Fix direction: either add a real migration that backfills/normalises the images key in old messages.content rows, or explicitly own the decision with a documented rollout-shim comment referencing this task ID (mirroring the worktree_path/03001 pattern). Decide whether empty-images-on-old-row is acceptable silent default or should surface a loud error.
