`SteerEntry` is persisted as an unversioned JSON blob in
`conversations.steering_queue`, and both read paths swallow deserialization
failure silently — a latent schema-evolution trap plus a current
error-is-silenced violation.

## Verified locations
- crates/phoenix-ide/src/state_machine/event.rs:9-16 — `struct SteerEntry`
  with zero serde annotations (no `deny_unknown_fields`, no
  `#[serde(default)]`). Fields: text, llm_text, images (`Vec<ImageData>`),
  message_id, user_agent, skill_invocation (`Option<SkillInvocation>`).
- crates/phoenix-ide/src/db.rs:234 — column added correctly via
  `ALTER TABLE conversations ADD COLUMN steering_queue TEXT NOT NULL DEFAULT '[]'`.
- crates/phoenix-ide/src/db.rs:674-675 — `serde_json::from_str(&queue_str).unwrap_or_default()`
- crates/phoenix-ide/src/db.rs:2026-2031 — `.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()`
  (the main conversation-load path).

## Why it matters
1. Current violation — silent loss. Both read paths turn a deserialization
   failure into an empty `Vec`, with no log line. A genuinely corrupt
   `steering_queue` row today loses the user's entire pending steering queue
   with no trace. Contrast `messages.content`, whose read path
   (`MessageContent::from_json(...).unwrap_or_else(|_| MessageContent::error(...))`,
   db.rs:2106) substitutes a *visible* error rather than vanishing.
2. Latent trap — schema evolution. With zero serde annotations, adding any
   non-`Option`/non-`serde(default)` field later fails deserialization of
   every already-persisted row, and #1 then drops every queue silently. There
   is no comment owning the JSON-in-TEXT decision or freezing the blob shape —
   contrast the sibling `ToolOutcome::images` (db/schema.rs:538-544), which
   carries a paragraph owning exactly this decision and cites task 13023.

## Recommended fix — Option B (versioned envelope, keep the blob)

Do NOT normalize into a `steering_entries` table. The data is tree-shaped
(entry -> `Vec<ImageData>`, entry -> `SkillInvocation`), the queue is transient
and always read whole (never queried across entries), and the codebase
deliberately stores message-shaped content as JSON-in-TEXT
(`messages.content`). A relational table would be the odd one out and is
normalization for its own sake here.

Instead:
- Wrap the persisted value in a versioned envelope, e.g.
  `#[serde(tag = "v")] enum SteeringQueue { V1 { entries: Vec<SteerEntryV1> } }`.
  A future `V2` then forces a new match arm — old rows structurally cannot be
  forgotten. This is the schema-evolution guarantee without a migration.
- Make both read paths (db.rs:674, db.rs:2030) return/propagate `Result` and
  surface failure loudly — at minimum `tracing::warn!` on the `Err` branch
  before any default, mirroring how `messages` degrades to a visible error.
- The `'[]'` column default must still parse as an empty V1 envelope — pick an
  envelope encoding where the empty case is representable, or keep a thin
  back-compat shim that reads a bare `[]` as `V1 { entries: [] }` during
  rollout.

The minimal, unambiguous slice (worth doing even alone): add the `tracing::warn!`
on the swallow at db.rs:674 and db.rs:2030 so future loss is diagnosable.

## Option A (rejected, recorded for completeness)
A normalized `steering_entries` table (+ `steering_entry_images` side table,
since `images` is one-to-many) with a `json_each` backfill migration. Gives
`ALTER TABLE` schema evolution and NOT NULL/FK constraints, but is 2-3 tables
for a transient FIFO and `skill_invocation` stays a blob regardless. Revisit
only if steering entries grow cross-conversation query needs or blob image
size becomes a measured performance problem.

## Related
- 13023 (toolcontent-images-serde-default-no-migr) — sibling JSON-in-TEXT
  field that DID get an owned backward-compat decision; the pattern to follow.
- 13027/13028/13029 — same code-correctness audit round.
