-- Questions the Coordinator posts to the user without stopping its own work
-- (REQ-AUQ-010). A question belongs to the Coordinator's stable
-- ProductConversation, not to the transcript that asked it, so it survives
-- continuation and transcript deletion. It stays open until the user answers or
-- dismisses it, or the Coordinator withdraws it. An answer or dismissal is
-- delivered back to the current Coordinator transcript exactly once;
-- delivered_at_us records that.
CREATE TABLE coordinator_user_questions (
    request_id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(request_id)) > 0),
    coordinator_product_conversation_id TEXT NOT NULL,
    questions TEXT NOT NULL CHECK (json_valid(questions) AND json_type(questions) = 'array'),
    asked_at_us INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'answered', 'dismissed', 'withdrawn')),
    settled_at_us INTEGER,
    reply TEXT,
    delivered_at_us INTEGER,
    CHECK ((status = 'open') = (settled_at_us IS NULL)),
    CHECK ((status IN ('answered', 'dismissed')) = (reply IS NOT NULL)),
    CHECK (delivered_at_us IS NULL OR status IN ('answered', 'dismissed'))
);

CREATE INDEX coordinator_user_questions_open
    ON coordinator_user_questions (asked_at_us)
    WHERE status = 'open';

CREATE INDEX coordinator_user_questions_undelivered
    ON coordinator_user_questions (settled_at_us)
    WHERE status IN ('answered', 'dismissed') AND delivered_at_us IS NULL;

CREATE TRIGGER coordinator_user_questions_validate BEFORE INSERT ON coordinator_user_questions
WHEN NOT EXISTS (SELECT 1 FROM product_conversations
                 WHERE id = NEW.coordinator_product_conversation_id AND kind = 'coordinator')
BEGIN SELECT RAISE(ABORT, 'question requires the coordinator ProductConversation'); END;

CREATE TRIGGER coordinator_user_questions_owner_deleted
BEFORE DELETE ON product_conversations
WHEN OLD.kind = 'coordinator'
BEGIN
    UPDATE coordinator_user_questions
    SET status = 'withdrawn', settled_at_us = CAST(unixepoch('subsec') * 1000000 AS INTEGER)
    WHERE coordinator_product_conversation_id = OLD.id AND status = 'open';
END;
