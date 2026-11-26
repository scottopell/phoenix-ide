CREATE TABLE native_close_initial_result_acknowledgements (
    request_id TEXT NOT NULL PRIMARY KEY,
    request_fingerprint TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    run_ordinal INTEGER NOT NULL CHECK (run_ordinal = 1),
    assistant_message_id TEXT NOT NULL CHECK (length(trim(assistant_message_id)) > 0),
    result_message_id TEXT NOT NULL CHECK (length(trim(result_message_id)) > 0),
    FOREIGN KEY (request_id, request_fingerprint, attempt_id, run_ordinal)
        REFERENCES native_close_initial_receipts(request_id, request_fingerprint, attempt_id, run_ordinal)
        ON DELETE CASCADE
);
CREATE TRIGGER native_close_initial_result_acknowledgements_no_update
BEFORE UPDATE ON native_close_initial_result_acknowledgements
BEGIN
    SELECT RAISE(ABORT, 'native Close result acknowledgements are immutable');
END;
CREATE TRIGGER native_close_initial_result_acknowledgements_exact_initial
BEFORE INSERT ON native_close_initial_result_acknowledgements
WHEN NOT EXISTS (
    SELECT 1 FROM native_close_initial_receipts receipt
    JOIN close_runs run ON run.attempt_id = receipt.attempt_id AND run.run_ordinal = 1
    JOIN close_obligations obligation ON obligation.attempt_id = receipt.attempt_id
    JOIN close_completion_retry_allowances allowance ON allowance.attempt_id = receipt.attempt_id
    WHERE receipt.request_id = NEW.request_id
      AND receipt.request_fingerprint = NEW.request_fingerprint
      AND receipt.attempt_id = NEW.attempt_id
      AND receipt.run_ordinal = 1 AND receipt.intent = 'initial'
      AND receipt.source_message_id = NEW.assistant_message_id
      AND run.status = 'stopped' AND obligation.phase = 'completed' AND obligation.native_managed = 1
      AND allowance.suppressed = 0
      AND EXISTS (SELECT 1 FROM messages assistant
                  WHERE assistant.conversation_id = receipt.source_transcript_id
                    AND assistant.message_id = NEW.assistant_message_id)
      AND EXISTS (SELECT 1 FROM messages result
                  WHERE result.conversation_id = receipt.source_transcript_id
                    AND result.message_id = NEW.result_message_id)
)
BEGIN
    SELECT RAISE(ABORT, 'native Close acknowledgement requires exact initial receipt');
END;
CREATE TRIGGER native_close_initial_result_acknowledgements_no_replacement
BEFORE INSERT ON native_close_initial_result_acknowledgements
WHEN EXISTS (SELECT 1 FROM native_close_initial_result_acknowledgements WHERE request_id = NEW.request_id)
BEGIN
    SELECT RAISE(ABORT, 'native Close result acknowledgement already exists');
END;
CREATE TRIGGER native_close_initial_result_acknowledgements_no_direct_delete
BEFORE DELETE ON native_close_initial_result_acknowledgements
WHEN EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'native Close result acknowledgements cannot be deleted');
END;
