-- Questions the Coordinator posts to the user without stopping its own work
-- (REQ-AUQ-010). A question stays open until the user answers or dismisses it,
-- or the Coordinator withdraws it. An answer or dismissal is delivered back to
-- the Coordinator transcript exactly once; delivered_at_us records that.
CREATE TABLE coordinator_user_questions (
    request_id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(request_id)) > 0),
    coordinator_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
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
