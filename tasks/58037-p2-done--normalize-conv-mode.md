Normalize the `conversations.conv_mode` JSON-TEXT column into real columns,
the final item from the JSON-in-TEXT audit and the highest-payoff one (four
migrations — 001, 002, 007, 021 — reach into the blob via json_extract/json_set).

ConvMode is a 4-variant tagged union (Explore/Direct/Work/Branch). Target: a
`conv_mode_kind` discriminator column + promoted field columns (branch_name,
worktree_path, base_branch, task_id, task_title, next_taskmd_id_hint), so SQL
filters/reads columns instead of json paths. Keep the in-memory/wire ConvMode
enum unchanged (parse rebuilds it from columns; the wire shape is identical).

Open design question (SQLite ALTER limits): a cross-column per-mode CHECK
(invalid variant combos unrepresentable) cannot be ADDed to the existing
conversations table without a full table rebuild, which is risky for the
central table (inbound FKs + the per-migration-transaction framework). Likely
land columns + Rust-enforced ConvMode invariant; drop the blob column (it would
otherwise hold a wrong default for non-Explore rows).
