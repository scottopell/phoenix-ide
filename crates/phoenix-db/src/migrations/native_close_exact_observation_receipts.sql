-- The historical table name is retained; this is the only native receipt store.
CREATE TABLE native_close_receipts_v160 (
    request_id TEXT NOT NULL PRIMARY KEY
        CHECK (typeof(request_id) = 'text' AND length(trim(request_id)) > 0),
    source_transcript_id TEXT NOT NULL
        CHECK (typeof(source_transcript_id) = 'text' AND length(trim(source_transcript_id)) > 0),
    source_message_id TEXT NOT NULL
        CHECK (typeof(source_message_id) = 'text' AND length(trim(source_message_id)) > 0),
    source_tool_use_id TEXT NOT NULL
        CHECK (typeof(source_tool_use_id) = 'text' AND length(trim(source_tool_use_id)) > 0),
    request_fingerprint TEXT NOT NULL
        CHECK (typeof(request_fingerprint) = 'text' AND length(trim(request_fingerprint)) > 0),
    product_conversation_id TEXT NOT NULL
        CHECK (typeof(product_conversation_id) = 'text' AND length(trim(product_conversation_id)) > 0),
    attempt_id TEXT NOT NULL
        CHECK (typeof(attempt_id) = 'text' AND length(trim(attempt_id)) > 0),
    run_ordinal INTEGER NOT NULL CHECK (typeof(run_ordinal) = 'integer' AND run_ordinal > 0),
    intent TEXT NOT NULL DEFAULT 'initial' CHECK (intent IN ('initial', 'observe')),
    CHECK (intent <> 'initial' OR run_ordinal = 1),
    UNIQUE (source_transcript_id, source_message_id, source_tool_use_id),
    FOREIGN KEY (attempt_id, product_conversation_id)
        REFERENCES close_obligations(attempt_id, product_conversation_id) ON DELETE CASCADE,
    FOREIGN KEY (attempt_id, run_ordinal)
        REFERENCES close_runs(attempt_id, run_ordinal) ON DELETE CASCADE
);

INSERT INTO native_close_receipts_v160
    (request_id, source_transcript_id, source_message_id, source_tool_use_id,
     request_fingerprint, product_conversation_id, attempt_id, run_ordinal, intent)
SELECT request_id, source_transcript_id, source_message_id, source_tool_use_id,
       request_fingerprint, product_conversation_id, attempt_id, run_ordinal, 'initial'
FROM native_close_initial_receipts;

DROP TABLE native_close_initial_receipts;
ALTER TABLE native_close_receipts_v160 RENAME TO native_close_initial_receipts;

CREATE TRIGGER native_close_initial_receipts_require_native_parent
BEFORE INSERT ON native_close_initial_receipts
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = NEW.attempt_id
      AND product_conversation_id = NEW.product_conversation_id
      AND native_managed = 1
)
BEGIN
    SELECT RAISE(ABORT, 'native Close receipt requires native-managed obligation');
END;

CREATE TRIGGER native_close_initial_receipts_immutable
BEFORE UPDATE ON native_close_initial_receipts
BEGIN
    SELECT RAISE(ABORT, 'native Close receipt is immutable');
END;

CREATE TRIGGER native_close_initial_receipts_no_replacement
BEFORE INSERT ON native_close_initial_receipts
WHEN EXISTS (SELECT 1 FROM native_close_initial_receipts WHERE request_id = NEW.request_id)
  OR EXISTS (SELECT 1 FROM native_close_initial_receipts
             WHERE source_transcript_id = NEW.source_transcript_id
               AND source_message_id = NEW.source_message_id
               AND source_tool_use_id = NEW.source_tool_use_id)
BEGIN
    SELECT RAISE(ABORT, 'native Close receipt cannot be replaced');
END;

CREATE TRIGGER native_close_initial_receipts_no_direct_delete
BEFORE DELETE ON native_close_initial_receipts
WHEN EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'native Close receipt must be retained with its obligation');
END;
