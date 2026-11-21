-- REQ-AUQ-010..012: questions the Coordinator posts without stopping its work.
CREATE TABLE coordinator_user_questions (
    request_id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(request_id)) > 0),
    coordinator_product_conversation_id TEXT NOT NULL
        REFERENCES product_conversations(id) ON DELETE CASCADE,
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

-- REQ-AUQ-011: a reply is delivered by the insert that accepts it.
CREATE TRIGGER coordinator_question_reply_accept_turn AFTER INSERT ON durable_turns
WHEN NEW.origin_kind = 'user_api'
BEGIN
    UPDATE coordinator_user_questions
    SET delivered_at_us = CAST(unixepoch('subsec') * 1000000 AS INTEGER)
    WHERE request_id = NEW.client_turn_key AND delivered_at_us IS NULL
      AND status IN ('answered', 'dismissed')
      AND coordinator_product_conversation_id =
          (SELECT product_conversation_id FROM conversations WHERE id = NEW.conversation_id);
END;

CREATE TRIGGER coordinator_question_reply_accept_steering AFTER INSERT ON steering_messages
WHEN NEW.origin_kind = 'user_api'
BEGIN
    UPDATE coordinator_user_questions
    SET delivered_at_us = CAST(unixepoch('subsec') * 1000000 AS INTEGER)
    WHERE request_id = NEW.message_id AND delivered_at_us IS NULL
      AND status IN ('answered', 'dismissed')
      AND coordinator_product_conversation_id =
          (SELECT product_conversation_id FROM conversations WHERE id = NEW.conversation_id);
END;
