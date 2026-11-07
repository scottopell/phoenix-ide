CREATE TABLE federated_continuation_receipts (
    caller_instance_id TEXT NOT NULL CHECK(typeof(caller_instance_id) = 'text' AND length(caller_instance_id) > 0),
    first_message_id TEXT NOT NULL CHECK(typeof(first_message_id) = 'text' AND length(first_message_id) > 0),
    request_digest TEXT NOT NULL CHECK(typeof(request_digest) = 'text' AND length(request_digest) = 64
        AND request_digest NOT GLOB '*[^0-9a-f]*'),
    product_conversation_id TEXT NOT NULL CHECK(typeof(product_conversation_id) = 'text' AND length(product_conversation_id) > 0),
    predecessor_transcript_id TEXT NOT NULL CHECK(typeof(predecessor_transcript_id) = 'text' AND length(predecessor_transcript_id) > 0),
    summary_message_id TEXT NOT NULL CHECK(typeof(summary_message_id) = 'text' AND length(summary_message_id) > 0),
    successor_transcript_id TEXT NOT NULL CHECK(typeof(successor_transcript_id) = 'text' AND length(successor_transcript_id) > 0),
    PRIMARY KEY (caller_instance_id, first_message_id),
    UNIQUE (first_message_id),
    UNIQUE (predecessor_transcript_id),
    UNIQUE (successor_transcript_id)
);

CREATE TRIGGER federated_continuation_receipts_immutable
BEFORE UPDATE ON federated_continuation_receipts
BEGIN
    SELECT RAISE(ABORT, 'federated continuation receipts are immutable');
END;

CREATE TRIGGER federated_continuation_receipts_reject_delete
BEFORE DELETE ON federated_continuation_receipts
BEGIN
    SELECT RAISE(ABORT, 'federated continuation receipts are immutable');
END;
