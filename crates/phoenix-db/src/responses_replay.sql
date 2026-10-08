CREATE TABLE IF NOT EXISTS active_responses_replay_sets (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    response_id TEXT NOT NULL CHECK(length(response_id) > 0),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    model TEXT NOT NULL CHECK(length(model) > 0),
    owner_message_id TEXT NOT NULL CHECK(length(owner_message_id) > 0),
    public_content TEXT NOT NULL,
    PRIMARY KEY(conversation_id, response_id),
    UNIQUE(conversation_id, ordinal),
    UNIQUE(conversation_id, owner_message_id)
);
CREATE TABLE IF NOT EXISTS active_responses_replay_items (
    conversation_id TEXT NOT NULL,
    response_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    payload TEXT NOT NULL,
    PRIMARY KEY(conversation_id, response_id, ordinal),
    FOREIGN KEY(conversation_id, response_id) REFERENCES active_responses_replay_sets(conversation_id, response_id) ON DELETE CASCADE
);
