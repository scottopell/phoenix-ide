CREATE TABLE coordinator_watch_ending_evidence (
    event_id TEXT PRIMARY KEY NOT NULL REFERENCES coordinator_watch_events(event_id) ON DELETE CASCADE,
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
           (input_message_id IS NOT NULL AND length(trim(input_message_id)) > 0)),
    CHECK ((origin_source_message_id IS NULL AND origin_source_tool_use_id IS NULL) OR
           (origin_kind IN ('internal_conversation','remote_conversation','remote_coordinator')
            AND origin_source_message_id IS NOT NULL AND origin_source_tool_use_id IS NOT NULL)),
    CHECK ((origin_kind IS NULL OR origin_kind IN ('unknown_historical','user_api','system_generated'))
             AND origin_product_conversation_id IS NULL AND origin_transcript_id IS NULL
             AND origin_subscription_event_id IS NULL AND origin_source_instance_id IS NULL
             AND origin_slack_request_id IS NULL AND origin_reminder_id IS NULL
             AND origin_source_message_id IS NULL AND origin_source_tool_use_id IS NULL
        OR origin_kind IS NOT NULL AND (
          origin_kind = 'internal_conversation'
            AND origin_product_conversation_id IS NOT NULL AND origin_transcript_id IS NOT NULL
            AND origin_subscription_event_id IS NULL AND origin_source_instance_id IS NULL
            AND origin_slack_request_id IS NULL AND origin_reminder_id IS NULL
        OR origin_kind IN ('remote_conversation','remote_coordinator')
            AND origin_product_conversation_id IS NOT NULL AND origin_transcript_id IS NOT NULL
            AND origin_source_instance_id IS NOT NULL AND origin_subscription_event_id IS NULL
            AND origin_slack_request_id IS NULL AND origin_reminder_id IS NULL
        OR origin_kind = 'remote_api' AND origin_source_instance_id IS NOT NULL
            AND origin_product_conversation_id IS NULL AND origin_transcript_id IS NULL
            AND origin_subscription_event_id IS NULL AND origin_slack_request_id IS NULL AND origin_reminder_id IS NULL
        OR origin_kind = 'subscription_event' AND origin_subscription_event_id IS NOT NULL
            AND origin_product_conversation_id IS NULL AND origin_transcript_id IS NULL
            AND origin_source_instance_id IS NULL AND origin_slack_request_id IS NULL AND origin_reminder_id IS NULL
        OR origin_kind = 'slack_gateway' AND origin_slack_request_id IS NOT NULL
            AND origin_product_conversation_id IS NULL AND origin_transcript_id IS NULL
            AND origin_source_instance_id IS NULL AND origin_subscription_event_id IS NULL AND origin_reminder_id IS NULL
        OR origin_kind = 'scheduled_reminder' AND origin_reminder_id IS NOT NULL
            AND origin_product_conversation_id IS NULL AND origin_transcript_id IS NULL
            AND origin_source_instance_id IS NULL AND origin_subscription_event_id IS NULL AND origin_slack_request_id IS NULL
        ))
);

CREATE TRIGGER coordinator_watch_ending_evidence_immutable
BEFORE UPDATE ON coordinator_watch_ending_evidence
BEGIN SELECT RAISE(ABORT, 'ending event evidence is immutable'); END;
