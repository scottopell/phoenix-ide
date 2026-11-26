CREATE UNIQUE INDEX conversations_transcript_owner_unique
    ON conversations(id, product_conversation_id);

-- Live ToolExecuting sources do not yet have a finalized messages row.
CREATE TABLE native_task_approval_source_calls (
    source_transcript_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    source_message_id TEXT NOT NULL CHECK (length(trim(source_message_id)) > 0),
    source_tool_use_id TEXT NOT NULL CHECK (length(trim(source_tool_use_id)) > 0),
    PRIMARY KEY (source_transcript_id, source_message_id, source_tool_use_id)
);

CREATE TABLE native_task_approval_receipts (
    source_transcript_id TEXT NOT NULL,
    source_message_id TEXT NOT NULL,
    source_tool_use_id TEXT NOT NULL,
    request_sha256 BLOB NOT NULL
        CHECK (typeof(request_sha256) = 'blob' AND length(request_sha256) = 32),
    target_product_conversation_id TEXT NOT NULL,
    target_transcript_id TEXT NOT NULL,
    proposal_message_id TEXT NOT NULL,
    proposal_tool_use_id TEXT NOT NULL CHECK (length(trim(proposal_tool_use_id)) > 0),
    approval_message_id TEXT NOT NULL UNIQUE,
    continuation_requested INTEGER NOT NULL CHECK (continuation_requested = 1),
    committed_at_us INTEGER NOT NULL
        CHECK (typeof(committed_at_us) = 'integer' AND committed_at_us >= 0),
    PRIMARY KEY (source_transcript_id, source_message_id, source_tool_use_id),
    UNIQUE (target_transcript_id, proposal_message_id, proposal_tool_use_id),
    FOREIGN KEY (source_transcript_id, source_message_id, source_tool_use_id)
        REFERENCES native_task_approval_source_calls
            (source_transcript_id, source_message_id, source_tool_use_id) ON DELETE CASCADE,
    FOREIGN KEY (target_transcript_id, target_product_conversation_id)
        REFERENCES conversations(id, product_conversation_id) ON DELETE CASCADE,
    FOREIGN KEY (target_transcript_id, proposal_message_id)
        REFERENCES messages(conversation_id, message_id) ON DELETE CASCADE,
    FOREIGN KEY (target_transcript_id, proposal_message_id)
        REFERENCES canonical_tool_round_receipts(conversation_id, assistant_message_id)
        ON DELETE CASCADE,
    FOREIGN KEY (target_transcript_id, approval_message_id)
        REFERENCES messages(conversation_id, message_id) ON DELETE CASCADE
);

CREATE TRIGGER native_task_approval_source_call_immutable
BEFORE UPDATE ON native_task_approval_source_calls
BEGIN SELECT RAISE(ABORT, 'native task approval source call is immutable'); END;

CREATE TRIGGER native_task_approval_receipt_immutable
BEFORE UPDATE ON native_task_approval_receipts
BEGIN SELECT RAISE(ABORT, 'native task approval receipt is immutable'); END;
