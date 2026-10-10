CREATE TABLE IF NOT EXISTS conversation_tool_definitions (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    input_schema TEXT NOT NULL,
    defer_loading INTEGER NOT NULL CHECK(defer_loading IN (0,1)),
    PRIMARY KEY(conversation_id, name)
);
CREATE TABLE IF NOT EXISTS conversation_callable_tools (
    conversation_id TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY(conversation_id, name),
    FOREIGN KEY(conversation_id, name) REFERENCES conversation_tool_definitions(conversation_id, name) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS conversation_tool_contexts (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    continuation_id TEXT NOT NULL UNIQUE,
    route_key TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS conversation_tool_context_initial (
    conversation_id TEXT NOT NULL REFERENCES conversation_tool_contexts(conversation_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    input_schema TEXT NOT NULL,
    defer_loading INTEGER NOT NULL CHECK(defer_loading IN (0,1)),
    PRIMARY KEY(conversation_id, ordinal),
    UNIQUE(conversation_id, name)
);
CREATE TABLE IF NOT EXISTS conversation_tool_context_changes (
    conversation_id TEXT NOT NULL REFERENCES conversation_tool_contexts(conversation_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    after_message_id TEXT NOT NULL REFERENCES messages(message_id),
    kind TEXT NOT NULL CHECK(kind IN ('addition','removal')),
    name TEXT NOT NULL,
    description TEXT,
    input_schema TEXT,
    defer_loading INTEGER CHECK(defer_loading IN (0,1)),
    PRIMARY KEY(conversation_id, ordinal),
    CHECK((kind = 'removal' AND description IS NULL AND input_schema IS NULL AND defer_loading IS NULL)
       OR (kind = 'addition' AND description IS NOT NULL AND input_schema IS NOT NULL AND defer_loading IS NOT NULL))
);
