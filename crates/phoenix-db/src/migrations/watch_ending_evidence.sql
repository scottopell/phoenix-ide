CREATE TABLE coordinator_watch_ending_evidence (
    event_id TEXT PRIMARY KEY REFERENCES coordinator_watch_events(event_id) ON DELETE CASCADE,
    response_state TEXT NOT NULL CHECK(response_state IN ('captured', 'unavailable', 'no_carrier')),
    response_message_id TEXT,
    digest_version INTEGER,
    response_digest TEXT,
    excerpt TEXT,
    prose_bytes INTEGER,
    capture_limit INTEGER,
    input_message_id TEXT,
    origin_kind TEXT,
    origin_product_conversation_id TEXT,
    origin_transcript_id TEXT,
    origin_subscription_event_id TEXT,
    origin_source_instance_id TEXT,
    origin_slack_request_id TEXT,
    origin_reminder_id TEXT,
    origin_source_message_id TEXT,
    origin_source_tool_use_id TEXT,
    CHECK ((response_state = 'captured' AND response_message_id IS NOT NULL
            AND digest_version IS NOT NULL AND digest_version = 1
            AND response_digest IS NOT NULL AND length(response_digest) = 64
            AND response_digest NOT GLOB '*[^0-9a-f]*'
            AND prose_bytes IS NOT NULL AND prose_bytes >= 0
            AND capture_limit IS NOT NULL AND capture_limit = 1024
            AND ((prose_bytes = 0 AND excerpt IS NULL) OR
                 (prose_bytes > 0 AND excerpt IS NOT NULL AND length(CAST(excerpt AS BLOB)) <= capture_limit)))
        OR (response_state = 'unavailable' AND response_message_id IS NOT NULL
            AND digest_version IS NULL AND response_digest IS NULL AND excerpt IS NULL
            AND prose_bytes IS NULL AND capture_limit IS NULL)
        OR (response_state = 'no_carrier' AND response_message_id IS NULL
            AND digest_version IS NULL AND response_digest IS NULL AND excerpt IS NULL
            AND prose_bytes IS NULL AND capture_limit IS NULL)),
    CHECK ((input_message_id IS NULL AND origin_kind IS NULL) OR
           (input_message_id IS NOT NULL AND origin_kind IS NOT NULL))
);

CREATE TRIGGER coordinator_watch_ending_evidence_immutable
BEFORE UPDATE ON coordinator_watch_ending_evidence
BEGIN SELECT RAISE(ABORT, 'ending event evidence is immutable'); END;
