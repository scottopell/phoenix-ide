CREATE TABLE slack_gateway_trigger_emojis (
    name TEXT PRIMARY KEY NOT NULL
        CHECK (typeof(name)='text' AND length(name) BETWEEN 1 AND 64
            AND name NOT GLOB '*[^a-z0-9_+-]*' AND name <> 'eyes'),
    ordinal INTEGER NOT NULL UNIQUE
        CHECK (typeof(ordinal)='integer' AND ordinal BETWEEN 0 AND 7)
) WITHOUT ROWID;

INSERT INTO slack_gateway_trigger_emojis(name, ordinal) VALUES ('fire', 0);
