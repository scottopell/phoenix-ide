ALTER TABLE active_responses_replay_items RENAME TO previous_responses_replay_items;
ALTER TABLE active_responses_replay_sets RENAME TO previous_responses_replay_sets;
CREATE TABLE active_responses_replay_sets (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    response_id TEXT NOT NULL CHECK(length(response_id) > 0),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    model TEXT NOT NULL CHECK(length(model) > 0),
    owner_message_id TEXT NOT NULL CHECK(length(owner_message_id) > 0),
    public_content TEXT NOT NULL,
    api_format TEXT NOT NULL CHECK (api_format IN ('responses', 'chat_completions')),
    PRIMARY KEY(conversation_id, response_id),
    UNIQUE(conversation_id, ordinal),
    UNIQUE(conversation_id, owner_message_id)
);
CREATE TABLE active_responses_replay_items (
    conversation_id TEXT NOT NULL,
    response_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    payload TEXT NOT NULL,
    PRIMARY KEY(conversation_id, response_id, ordinal),
    FOREIGN KEY(conversation_id, response_id) REFERENCES active_responses_replay_sets(conversation_id, response_id) ON DELETE CASCADE
);
INSERT INTO active_responses_replay_sets
    SELECT conversation_id, response_id, ordinal, model, owner_message_id, public_content, 'responses'
    FROM previous_responses_replay_sets;
INSERT INTO active_responses_replay_items SELECT * FROM previous_responses_replay_items;
DROP TABLE previous_responses_replay_items;
DROP TABLE previous_responses_replay_sets;

DROP TRIGGER canonical_tool_round_receipt_source;
ALTER TABLE canonical_tool_round_receipts RENAME TO previous_canonical_tool_round_receipts;
CREATE TABLE canonical_tool_round_receipts (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    assistant_message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
    selected_checkpoint_sha256 BLOB NOT NULL
        CHECK (typeof(selected_checkpoint_sha256) = 'blob' AND length(selected_checkpoint_sha256) = 32),
    selected_state_updated_at_us INTEGER NOT NULL
        CHECK (typeof(selected_state_updated_at_us) = 'integer'
               AND selected_state_updated_at_us BETWEEN 0 AND 9223372036854775807),
    replay_kind TEXT NOT NULL CHECK (replay_kind IN ('none','clear','anthropic','responses','chat_completions')),
    replay_payload_sha256 BLOB,
    PRIMARY KEY (conversation_id, assistant_message_id),
    CHECK ((replay_kind IN ('none','clear') AND replay_payload_sha256 IS NULL)
        OR (replay_kind IN ('anthropic','responses','chat_completions') AND replay_payload_sha256 IS NOT NULL
            AND typeof(replay_payload_sha256) = 'blob' AND length(replay_payload_sha256) = 32))
);
INSERT INTO canonical_tool_round_receipts SELECT * FROM previous_canonical_tool_round_receipts;
DROP TABLE previous_canonical_tool_round_receipts;
CREATE TRIGGER canonical_tool_round_receipt_source
BEFORE INSERT ON canonical_tool_round_receipts
WHEN NOT EXISTS (
    SELECT 1 FROM messages WHERE message_id = NEW.assistant_message_id
        AND conversation_id = NEW.conversation_id AND message_type = 'agent'
)
BEGIN SELECT RAISE(ABORT, 'canonical tool round source mismatch'); END;
