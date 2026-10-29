CREATE TABLE canonical_tool_round_receipts (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    assistant_message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
    selected_checkpoint_sha256 BLOB NOT NULL
        CHECK (typeof(selected_checkpoint_sha256) = 'blob' AND length(selected_checkpoint_sha256) = 32),
    selected_state_updated_at_us INTEGER NOT NULL
        CHECK (typeof(selected_state_updated_at_us) = 'integer'
               AND selected_state_updated_at_us BETWEEN 0 AND 9223372036854775807),
    replay_kind TEXT NOT NULL CHECK (replay_kind IN ('none','clear','anthropic','responses')),
    replay_payload_sha256 BLOB,
    PRIMARY KEY (conversation_id, assistant_message_id),
    CHECK ((replay_kind IN ('none','clear') AND replay_payload_sha256 IS NULL)
        OR (replay_kind IN ('anthropic','responses') AND replay_payload_sha256 IS NOT NULL
            AND typeof(replay_payload_sha256) = 'blob' AND length(replay_payload_sha256) = 32))
);

CREATE TRIGGER canonical_tool_round_receipt_source
BEFORE INSERT ON canonical_tool_round_receipts
WHEN NOT EXISTS (
    SELECT 1 FROM messages WHERE message_id = NEW.assistant_message_id
        AND conversation_id = NEW.conversation_id AND message_type = 'agent'
)
BEGIN SELECT RAISE(ABORT, 'canonical tool round source mismatch'); END;
