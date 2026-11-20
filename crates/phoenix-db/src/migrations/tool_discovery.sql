CREATE TABLE conversation_advertised_tools (
    conversation_id TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (conversation_id, name),
    FOREIGN KEY (conversation_id, name)
        REFERENCES conversation_tool_definitions(conversation_id, name) ON DELETE CASCADE
);
INSERT INTO conversation_advertised_tools SELECT conversation_id, name FROM conversation_callable_tools;

CREATE TABLE tool_discovery_results (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    owner_message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
    tool_call_id TEXT NOT NULL CHECK (length(tool_call_id) > 0),
    result_message_id TEXT NOT NULL UNIQUE REFERENCES messages(message_id) ON DELETE CASCADE,
    protocol TEXT NOT NULL CHECK (protocol IN ('function','openai_client')),
    PRIMARY KEY (conversation_id, owner_message_id, tool_call_id)
);
CREATE TABLE tool_discovery_result_tools (
    result_message_id TEXT NOT NULL REFERENCES tool_discovery_results(result_message_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    name TEXT NOT NULL CHECK (length(name) > 0),
    description TEXT NOT NULL,
    input_schema TEXT NOT NULL,
    defer_loading INTEGER NOT NULL CHECK (defer_loading IN (0,1)),
    PRIMARY KEY (result_message_id, ordinal),
    UNIQUE (result_message_id, name)
);
CREATE TRIGGER tool_discovery_result_source
BEFORE INSERT ON tool_discovery_results
WHEN NOT EXISTS (
    SELECT 1 FROM messages
    WHERE message_id = NEW.owner_message_id AND conversation_id = NEW.conversation_id AND message_type = 'agent'
) OR NOT EXISTS (
    SELECT 1 FROM messages
    WHERE message_id = NEW.result_message_id AND conversation_id = NEW.conversation_id AND message_type = 'tool'
)
BEGIN SELECT RAISE(ABORT, 'tool discovery result source mismatch'); END;
CREATE TRIGGER tool_discovery_result_immutable
BEFORE UPDATE ON tool_discovery_results
BEGIN SELECT RAISE(ABORT, 'tool discovery result is immutable'); END;
CREATE TRIGGER tool_discovery_definition_immutable
BEFORE UPDATE ON tool_discovery_result_tools
BEGIN SELECT RAISE(ABORT, 'tool discovery definition is immutable'); END;
