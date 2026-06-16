Normalize user/skill message attachments out of the `messages.content` JSON blob
into child tables, per the audit of JSON-in-TEXT columns and the locked AGENTS.md
rule "Persisted structure belongs in the schema, not in serde".

Today `UserContent.{files,images}` and `SkillContent.files` live inside the
`messages.content` TEXT blob. That forces serde(default)/skip shims, json_set
backfill migrations (014, 024 on the closed PR #300), and a `content LIKE
'%stored_path%'` attachment GC query. Child collections are never an earned blob.

Target design (backend-only; wire + UI unchanged):
- New tables `message_files(message_id, ordinal, original_name, media_type,
  size_bytes, stored_path)` and `message_images(message_id, ordinal, media_type,
  data)`, FK cascade on messages(message_id), PK (message_id, ordinal).
- The persisted `content` blob no longer carries files/images (single source of
  truth = child tables). `UserContent`/`SkillContent` keep the fields IN MEMORY
  (required, no serde default); the DB layer strips on write and reassembles on
  read.
- Wire stays byte-identical: `enrich_message_for_api` serializes the in-memory
  (reassembled) content, so the UI's `content.files`/`content.images` reads are
  unaffected.

Phases (each a commit in the PR):
1. Create child tables (DDL + migration) and backfill from the existing blob via
   json_each. Inert until the cutover. [this commit]
2. Cutover: write paths (add_message_with_seq{,_at}, insert_message_tx) write
   child rows + persist stripped content; read paths (get_messages{,_after},
   get_message_by_id, FTS reconcile) hydrate attachments; migration strips
   files/images from the blob; remove serde(default)/skip from the structs.
3. Repoint the attachment GC sweep (handlers.rs referenced_attachment_paths) at
   message_files instead of `content LIKE '%stored_path%'`.

Follow-ups tracked separately (from the same audit): messages.usage_data →
columns; steering_queue → child table; conv_mode discriminator + worktree cols.
