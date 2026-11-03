CREATE TABLE reminders (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    product_conversation_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    source_transcript_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    source_message_id TEXT NOT NULL CHECK (length(trim(source_message_id)) > 0),
    source_tool_use_id TEXT NOT NULL CHECK (length(trim(source_tool_use_id)) > 0),
    create_request_fingerprint TEXT NOT NULL CHECK (length(create_request_fingerprint) = 64
        AND create_request_fingerprint NOT GLOB '*[^0-9a-f]*'),
    text TEXT NOT NULL CHECK (length(trim(text)) > 0 AND length(text) <= 4096),
    due_at_us INTEGER NOT NULL CHECK (typeof(due_at_us) = 'integer' AND due_at_us >= 0),
    timezone TEXT NOT NULL CHECK (length(trim(timezone)) BETWEEN 1 AND 128),
    revision INTEGER NOT NULL CHECK (typeof(revision) = 'integer' AND revision >= 1),
    created_at_us INTEGER NOT NULL CHECK (typeof(created_at_us) = 'integer' AND created_at_us >= 0),
    state TEXT NOT NULL CHECK (state IN ('scheduled', 'due', 'cancelled')),
    occurred_at_us INTEGER CHECK (occurred_at_us IS NULL OR
        (typeof(occurred_at_us) = 'integer' AND occurred_at_us >= due_at_us)),
    cancelled_at_us INTEGER CHECK (cancelled_at_us IS NULL OR
        (typeof(cancelled_at_us) = 'integer' AND cancelled_at_us >= 0)),
    UNIQUE (source_transcript_id, source_message_id, source_tool_use_id),
    CHECK ((state = 'due') = (occurred_at_us IS NOT NULL)),
    CHECK ((state = 'cancelled') = (cancelled_at_us IS NOT NULL))
) STRICT;

CREATE INDEX reminders_scheduled_deadline ON reminders(due_at_us, id) WHERE state = 'scheduled';
CREATE INDEX reminders_owner ON reminders(product_conversation_id, created_at_us, id);

CREATE TRIGGER reminders_owner_insert BEFORE INSERT ON reminders
WHEN NOT EXISTS (
    SELECT 1 FROM conversations c JOIN product_conversations p ON p.id = c.product_conversation_id
    WHERE c.id = NEW.source_transcript_id AND c.product_conversation_id = NEW.product_conversation_id
      AND c.parent_conversation_id IS NULL
      AND ((p.kind = 'ordinary' AND c.runtime_role = 'user' AND p.ordinary_lifecycle = 'open')
        OR (p.kind = 'coordinator' AND c.runtime_role = 'coordinator' AND c.coordinator_head = 1))
)
BEGIN SELECT RAISE(ABORT, 'reminder requires an admitted top-level conversation'); END;

CREATE TRIGGER reminders_transition_update BEFORE UPDATE ON reminders
WHEN NEW.id IS NOT OLD.id OR NEW.product_conversation_id IS NOT OLD.product_conversation_id
  OR NEW.source_transcript_id IS NOT OLD.source_transcript_id
  OR NEW.source_message_id IS NOT OLD.source_message_id
  OR NEW.source_tool_use_id IS NOT OLD.source_tool_use_id
  OR NEW.create_request_fingerprint IS NOT OLD.create_request_fingerprint
  OR NEW.text IS NOT OLD.text OR NEW.created_at_us IS NOT OLD.created_at_us
  OR OLD.state <> 'scheduled'
  OR NOT (
    (NEW.state = 'scheduled' AND OLD.revision < 9223372036854775807 AND NEW.revision = OLD.revision + 1)
    OR (NEW.state IN ('due', 'cancelled') AND NEW.revision = OLD.revision
      AND NEW.due_at_us = OLD.due_at_us AND NEW.timezone = OLD.timezone)
  )
BEGIN SELECT RAISE(ABORT, 'invalid reminder transition'); END;

-- Add the source identity without rewriting existing provenance or retired wake rows.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema SET sql = replace(sql, '''slack_gateway''))', '''slack_gateway'', ''scheduled_reminder''))')
WHERE type = 'table' AND name IN ('messages','steering_messages','durable_turns');
PRAGMA writable_schema = RESET;
ALTER TABLE messages ADD COLUMN origin_reminder_id TEXT REFERENCES reminders(id);
CREATE TRIGGER messages_reminder_origin_insert BEFORE INSERT ON messages
WHEN (NEW.origin_kind = 'scheduled_reminder') != (NEW.origin_reminder_id IS NOT NULL AND length(trim(NEW.origin_reminder_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid reminder origin'); END;
CREATE TRIGGER messages_reminder_origin_update BEFORE UPDATE OF origin_kind, origin_reminder_id ON messages
WHEN (NEW.origin_kind = 'scheduled_reminder') != (NEW.origin_reminder_id IS NOT NULL AND length(trim(NEW.origin_reminder_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid reminder origin'); END;
ALTER TABLE steering_messages ADD COLUMN origin_reminder_id TEXT REFERENCES reminders(id);
CREATE TRIGGER steering_messages_reminder_origin_insert BEFORE INSERT ON steering_messages
WHEN (NEW.origin_kind = 'scheduled_reminder') != (NEW.origin_reminder_id IS NOT NULL AND length(trim(NEW.origin_reminder_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid reminder origin'); END;
CREATE TRIGGER steering_messages_reminder_origin_update BEFORE UPDATE OF origin_kind, origin_reminder_id ON steering_messages
WHEN (NEW.origin_kind = 'scheduled_reminder') != (NEW.origin_reminder_id IS NOT NULL AND length(trim(NEW.origin_reminder_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid reminder origin'); END;
ALTER TABLE durable_turns ADD COLUMN origin_reminder_id TEXT REFERENCES reminders(id);
CREATE TRIGGER durable_turns_reminder_origin_insert BEFORE INSERT ON durable_turns
WHEN (NEW.origin_kind = 'scheduled_reminder') != (NEW.origin_reminder_id IS NOT NULL AND length(trim(NEW.origin_reminder_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid reminder origin'); END;
CREATE TRIGGER durable_turns_reminder_origin_update BEFORE UPDATE OF origin_kind, origin_reminder_id ON durable_turns
WHEN (NEW.origin_kind = 'scheduled_reminder') != (NEW.origin_reminder_id IS NOT NULL AND length(trim(NEW.origin_reminder_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid reminder origin'); END;
