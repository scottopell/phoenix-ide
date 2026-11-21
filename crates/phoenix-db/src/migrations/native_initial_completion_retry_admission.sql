-- Existing receipts and adopted operations are not backfilled with native origin proof.
CREATE TABLE native_close_completion_retry_origins (
    attempt_id TEXT NOT NULL PRIMARY KEY,
    origin_request_id TEXT NOT NULL UNIQUE,
    origin_request_fingerprint TEXT NOT NULL CHECK (length(trim(origin_request_fingerprint)) > 0),
    source_transcript_id TEXT NOT NULL CHECK (length(trim(source_transcript_id)) > 0),
    source_message_id TEXT NOT NULL CHECK (length(trim(source_message_id)) > 0),
    source_tool_use_id TEXT NOT NULL CHECK (length(trim(source_tool_use_id)) > 0),
    product_conversation_id TEXT NOT NULL CHECK (length(trim(product_conversation_id)) > 0),
    initial_run_ordinal INTEGER NOT NULL CHECK (typeof(initial_run_ordinal) = 'integer' AND initial_run_ordinal = 1),
    UNIQUE (origin_request_id, origin_request_fingerprint, attempt_id),
    FOREIGN KEY (origin_request_id, origin_request_fingerprint, attempt_id, initial_run_ordinal)
        REFERENCES native_close_initial_receipts(request_id, request_fingerprint, attempt_id, run_ordinal)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (attempt_id, product_conversation_id)
        REFERENCES close_obligations(attempt_id, product_conversation_id)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TRIGGER native_close_completion_retry_origins_require_fresh_receipt
BEFORE INSERT ON native_close_completion_retry_origins
WHEN EXISTS (SELECT 1 FROM native_close_completion_retry_origins
             WHERE attempt_id = NEW.attempt_id OR origin_request_id = NEW.origin_request_id)
  OR EXISTS (SELECT 1 FROM native_close_initial_receipts
             WHERE request_id = NEW.origin_request_id OR attempt_id = NEW.attempt_id)
  OR EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = NEW.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'completion origin must precede its newly created operation, never adoption or replacement');
END;
CREATE TRIGGER native_close_completion_retry_receipt_requires_exact_origin
AFTER INSERT ON native_close_initial_receipts
WHEN NEW.intent = 'initial' AND NOT EXISTS (
    SELECT 1 FROM native_close_completion_retry_origins origin
    WHERE origin.attempt_id = NEW.attempt_id AND origin.origin_request_id = NEW.request_id
      AND origin.origin_request_fingerprint = NEW.request_fingerprint
      AND origin.source_transcript_id = NEW.source_transcript_id
      AND origin.source_message_id = NEW.source_message_id
      AND origin.source_tool_use_id = NEW.source_tool_use_id
      AND origin.product_conversation_id = NEW.product_conversation_id
      AND origin.initial_run_ordinal = NEW.run_ordinal
)
BEGIN
    SELECT RAISE(ABORT, 'initial receipt requires its exact completion origin');
END;

CREATE TABLE native_close_completion_retry_source_revocations (
    attempt_id TEXT NOT NULL PRIMARY KEY
        REFERENCES native_close_completion_retry_origins(attempt_id),
    revoked_at_us INTEGER NOT NULL CHECK (typeof(revoked_at_us) = 'integer' AND revoked_at_us >= 0)
);
CREATE TRIGGER native_close_completion_retry_source_revocations_no_replacement
BEFORE INSERT ON native_close_completion_retry_source_revocations
WHEN EXISTS (SELECT 1 FROM native_close_completion_retry_source_revocations WHERE attempt_id = NEW.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'completion source revocation cannot be replaced');
END;
CREATE TRIGGER native_close_completion_retry_source_revocations_immutable
BEFORE UPDATE ON native_close_completion_retry_source_revocations
BEGIN
    SELECT RAISE(ABORT, 'completion source revocation is immutable');
END;
CREATE TRIGGER native_close_completion_retry_source_revocations_no_delete
BEFORE DELETE ON native_close_completion_retry_source_revocations
BEGIN
    SELECT RAISE(ABORT, 'completion source revocation is retained');
END;

CREATE TRIGGER native_close_completion_retry_origins_immutable
BEFORE UPDATE ON native_close_completion_retry_origins
BEGIN
    SELECT RAISE(ABORT, 'native Close completion retry origin is immutable');
END;
CREATE TRIGGER native_close_completion_retry_origins_no_delete
BEFORE DELETE ON native_close_completion_retry_origins
BEGIN
    SELECT RAISE(ABORT, 'native Close completion retry origin is retained');
END;

-- The row is both the single-consumption record and exact automatic receipt. It is
-- separate from native_close_initial_receipts so automatic admission never asserts
-- that an explicit_retry source request occurred.
CREATE TABLE native_close_completion_retry_admissions (
    attempt_id TEXT NOT NULL PRIMARY KEY,
    origin_request_id TEXT NOT NULL UNIQUE,
    origin_request_fingerprint TEXT NOT NULL CHECK (length(trim(origin_request_fingerprint)) > 0),
    product_conversation_id TEXT NOT NULL CHECK (length(trim(product_conversation_id)) > 0),
    failed_run_ordinal INTEGER NOT NULL CHECK (typeof(failed_run_ordinal) = 'integer' AND failed_run_ordinal = 1),
    successor_run_ordinal INTEGER NOT NULL CHECK (typeof(successor_run_ordinal) = 'integer' AND successor_run_ordinal = 2),
    admitted_at_us INTEGER NOT NULL CHECK (typeof(admitted_at_us) = 'integer' AND admitted_at_us >= 0),
    UNIQUE (attempt_id, successor_run_ordinal),
    FOREIGN KEY (attempt_id) REFERENCES native_close_completion_retry_origins(attempt_id),
    FOREIGN KEY (origin_request_id, origin_request_fingerprint, attempt_id)
        REFERENCES native_close_completion_retry_origins(origin_request_id, origin_request_fingerprint, attempt_id),
    FOREIGN KEY (attempt_id, product_conversation_id)
        REFERENCES close_obligations(attempt_id, product_conversation_id),
    FOREIGN KEY (attempt_id, failed_run_ordinal)
        REFERENCES close_runs(attempt_id, run_ordinal),
    FOREIGN KEY (attempt_id, successor_run_ordinal)
        REFERENCES close_runs(attempt_id, run_ordinal) DEFERRABLE INITIALLY DEFERRED
);

CREATE VIEW native_close_completion_retry_initial_failures AS
SELECT origin.*, proof.inspection_generation, proof.inspection_fingerprint,
       failure.failure_occurrence_id, failure.occurred_at_us, failure.confirmed_at_us
FROM native_close_completion_retry_origins origin
JOIN native_close_initial_complete_retirement_proofs proof
  ON proof.request_id = origin.origin_request_id
 AND proof.request_fingerprint = origin.origin_request_fingerprint
 AND proof.attempt_id = origin.attempt_id
 AND proof.product_conversation_id = origin.product_conversation_id
 AND proof.source_transcript_id = origin.source_transcript_id
 AND proof.source_message_id = origin.source_message_id
 AND proof.source_tool_use_id = origin.source_tool_use_id
 AND proof.run_ordinal = origin.initial_run_ordinal
JOIN close_runs run ON run.attempt_id = origin.attempt_id AND run.run_ordinal = 1
JOIN close_cleanup_failures failure ON failure.attempt_id = run.attempt_id
 AND failure.cleanup_run_ordinal = run.run_ordinal
WHERE run.status = 'stopped' AND run.ended_at_us = failure.occurred_at_us
  AND failure.failure_occurrence_id = 'close-failure:1:' || run.attempt_id
  AND failure.source_product_conversation_id = origin.product_conversation_id
  AND failure.authority_kind = 'attempt_interrupted' AND failure.reason = 'interrupted'
  AND failure.stop_certainty = 'conversation_and_processes_stopped'
  AND failure.confirmed_at_us >= run.created_at_us
  AND failure.confirmed_at_us <= failure.occurred_at_us
  AND failure.inspection_generation IS NULL AND failure.inspection_fingerprint IS NULL
  AND failure.scope IS NULL AND failure.resource_kind IS NULL
  AND failure.identity_kind IS NULL AND failure.identity_codec IS NULL AND failure.identity_value IS NULL
  AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
                  WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
  AND EXISTS (SELECT 1 FROM coordinator_watch_events event
              WHERE event.event_id = failure.failure_occurrence_id
                AND event.mandatory_failure_occurrence_id = failure.failure_occurrence_id
                AND event.route_kind = 'mandatory_close_failure');

CREATE TRIGGER native_close_completion_retry_admission_requires_proof
BEFORE INSERT ON native_close_completion_retry_admissions
WHEN EXISTS (SELECT 1 FROM native_close_completion_retry_admissions
             WHERE attempt_id = NEW.attempt_id OR origin_request_id = NEW.origin_request_id)
  OR NOT EXISTS (
    SELECT 1 FROM native_close_completion_retry_initial_failures origin
    JOIN close_completion_retry_allowances budget ON budget.attempt_id = origin.attempt_id
    JOIN native_close_initial_complete_retirement_proofs proof
      ON proof.request_id = origin.origin_request_id
     AND proof.request_fingerprint = origin.origin_request_fingerprint
     AND proof.product_conversation_id = origin.product_conversation_id
     AND proof.attempt_id = origin.attempt_id
     AND proof.run_ordinal = NEW.failed_run_ordinal
    JOIN close_runs run ON run.attempt_id = proof.attempt_id AND run.run_ordinal = proof.run_ordinal
    JOIN close_cleanup_failures failure
      ON failure.attempt_id = run.attempt_id AND failure.cleanup_run_ordinal = run.run_ordinal
    JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
    WHERE origin.attempt_id = NEW.attempt_id
      AND origin.origin_request_id = NEW.origin_request_id
      AND origin.origin_request_fingerprint = NEW.origin_request_fingerprint
      AND origin.product_conversation_id = NEW.product_conversation_id
      AND budget.consumed = 0 AND budget.suppressed = 0
      AND NEW.admitted_at_us > origin.occurred_at_us
      AND NOT EXISTS (SELECT 1 FROM native_close_completion_retry_source_revocations revoked
                      WHERE revoked.attempt_id = origin.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = origin.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = origin.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success WHERE success.attempt_id = origin.attempt_id)
      AND NOT EXISTS (SELECT 1 FROM close_attempt_participants captured
                      JOIN durable_turns turn ON turn.conversation_id = captured.conversation_id
                      WHERE captured.attempt_id = origin.attempt_id
                        AND ((turn.owns_conversation = 1 AND turn.terminal_kind IS NULL)
                          OR EXISTS (SELECT 1 FROM direct_turn_terminal_obligations terminal
                                     WHERE terminal.turn_id = turn.turn_id)))
      AND NOT EXISTS (SELECT 1 FROM close_attempt_participants captured
                      JOIN conversation_creation_jobs creation ON creation.conversation_id = captured.conversation_id
                      WHERE captured.attempt_id = origin.attempt_id
                        AND (creation.status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling')
                          OR (creation.status = 'failed' AND EXISTS (
                              SELECT 1 FROM conversation_creation_resource_reservations reservation
                              WHERE reservation.job_id = creation.id AND reservation.status <> 'released'))))
      AND run.status = 'stopped'
      AND run.run_ordinal = (SELECT MAX(latest.run_ordinal) FROM close_runs latest WHERE latest.attempt_id = run.attempt_id)
      AND failure.authority_kind = 'attempt_interrupted'
      AND failure.reason = 'interrupted'
      AND failure.stop_certainty = 'conversation_and_processes_stopped'
      AND failure.confirmed_at_us IS NOT NULL
      AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
                      WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
      AND obligation.native_managed = 1
      AND obligation.product_conversation_id = proof.product_conversation_id
      AND obligation.phase = 'completed'
      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND NEW.failed_run_ordinal = 1 AND NEW.successor_run_ordinal = 2
      AND NOT EXISTS (SELECT 1 FROM close_runs later
                      WHERE later.attempt_id = run.attempt_id AND later.run_ordinal > run.run_ordinal)
)
BEGIN
    SELECT RAISE(ABORT, 'automatic completion retry requires exact initial all-success interruption proof and unused allowance');
END;

CREATE TRIGGER native_close_completion_retry_consume_allowance
AFTER INSERT ON native_close_completion_retry_admissions
BEGIN
    UPDATE close_completion_retry_allowances SET consumed = 1
    WHERE attempt_id = NEW.attempt_id AND consumed = 0 AND suppressed = 0;
    SELECT CASE WHEN changes() <> 1
        THEN RAISE(ABORT, 'automatic completion retry allowance is unavailable') END;
END;

CREATE TRIGGER native_close_completion_retry_admissions_immutable
BEFORE UPDATE ON native_close_completion_retry_admissions
BEGIN
    SELECT RAISE(ABORT, 'native Close completion retry admission is immutable');
END;
CREATE TRIGGER native_close_completion_retry_admissions_no_delete
BEFORE DELETE ON native_close_completion_retry_admissions
BEGIN
    SELECT RAISE(ABORT, 'native Close completion retry admission is retained');
END;

CREATE VIEW native_close_completion_retry_successors AS
SELECT admission.attempt_id, admission.product_conversation_id,
       admission.origin_request_id, admission.origin_request_fingerprint,
       origin.source_transcript_id, origin.source_message_id, origin.source_tool_use_id,
       origin.inspection_generation, origin.inspection_fingerprint,
       admission.failed_run_ordinal, admission.successor_run_ordinal,
       admission.admitted_at_us, run.status AS successor_status,
       run.created_at_us, run.ended_at_us, budget.suppressed
FROM native_close_completion_retry_admissions admission
JOIN native_close_completion_retry_initial_failures origin
  ON origin.attempt_id = admission.attempt_id
 AND origin.origin_request_id = admission.origin_request_id
 AND origin.origin_request_fingerprint = admission.origin_request_fingerprint
 AND origin.product_conversation_id = admission.product_conversation_id
 AND origin.initial_run_ordinal = admission.failed_run_ordinal
JOIN close_completion_retry_allowances budget ON budget.attempt_id = admission.attempt_id
JOIN close_runs run ON run.attempt_id = admission.attempt_id
                  AND run.run_ordinal = admission.successor_run_ordinal
WHERE admission.failed_run_ordinal = 1 AND admission.successor_run_ordinal = 2
  AND budget.consumed = 1
  AND run.retry_evidence_kind = 'verified_completion' AND run.retry_requested_by = 'global'
  AND run.created_at_us = admission.admitted_at_us
  AND run.retry_observed_at_us = admission.admitted_at_us
  AND admission.admitted_at_us > origin.occurred_at_us
  AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans plan WHERE plan.attempt_id = run.attempt_id)
  AND NOT EXISTS (SELECT 1 FROM native_close_initial_receipts receipt
                  WHERE receipt.attempt_id = run.attempt_id AND receipt.run_ordinal > 1)
  AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect WHERE effect.attempt_id = run.attempt_id)
  AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success WHERE success.attempt_id = run.attempt_id)
  AND NOT EXISTS (SELECT 1 FROM close_runs later
                  WHERE later.attempt_id = run.attempt_id AND later.run_ordinal > run.run_ordinal);

CREATE VIEW native_close_completion_retry_finalizable AS
SELECT successor.* FROM native_close_completion_retry_successors successor
JOIN close_obligations obligation ON obligation.attempt_id = successor.attempt_id
WHERE successor.successor_status = 'running' AND successor.suppressed = 0
  AND obligation.phase = 'completed'
  AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
  AND NOT EXISTS (SELECT 1 FROM native_close_completion_retry_source_revocations revoked
                  WHERE revoked.attempt_id = successor.attempt_id)
  AND NOT EXISTS (SELECT 1 FROM close_cleanup_failures failure
                  WHERE failure.attempt_id = successor.attempt_id AND failure.cleanup_run_ordinal = 2)
  AND NOT EXISTS (SELECT 1 FROM close_attempt_participants captured
                  JOIN durable_turns turn ON turn.conversation_id = captured.conversation_id
                  WHERE captured.attempt_id = successor.attempt_id
                    AND ((turn.owns_conversation = 1 AND turn.terminal_kind IS NULL)
                      OR EXISTS (SELECT 1 FROM direct_turn_terminal_obligations terminal WHERE terminal.turn_id = turn.turn_id)))
  AND NOT EXISTS (SELECT 1 FROM close_attempt_participants captured
                  JOIN conversation_creation_jobs creation ON creation.conversation_id = captured.conversation_id
                  WHERE captured.attempt_id = successor.attempt_id
                    AND (creation.status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling')
                      OR (creation.status = 'failed' AND EXISTS (
                          SELECT 1 FROM conversation_creation_resource_reservations reservation
                          WHERE reservation.job_id = creation.id AND reservation.status <> 'released'))));

CREATE TRIGGER native_close_completion_retry_no_resource_effects
BEFORE INSERT ON close_run_retry_effects
WHEN EXISTS (SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'automatic completion successor has no resource effects');
END;
CREATE TRIGGER native_close_completion_retry_no_resource_successes
BEFORE INSERT ON close_run_retry_successes
WHEN EXISTS (SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'automatic completion successor has no resource successes');
END;
CREATE TRIGGER native_close_completion_retry_no_process_successes
BEFORE INSERT ON close_process_step_successes
WHEN EXISTS (SELECT 1 FROM native_close_completion_retry_admissions
             WHERE attempt_id = NEW.attempt_id AND successor_run_ordinal = NEW.run_ordinal)
BEGIN
    SELECT RAISE(ABORT, 'automatic completion successor cannot perform process work');
END;
CREATE TRIGGER native_close_completion_retry_no_explicit_plan
BEFORE INSERT ON native_close_retry_plans
WHEN EXISTS (SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'automatic completion successor cannot become an explicit plan');
END;
CREATE TRIGGER native_close_completion_retry_no_successor_receipt
BEFORE INSERT ON native_close_initial_receipts
WHEN NEW.run_ordinal > 1 AND EXISTS (
    SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'automatic completion successor is not an explicit source request');
END;
CREATE TRIGGER native_close_completion_retry_no_failure_children
BEFORE INSERT ON close_cleanup_failure_resources
WHEN EXISTS (SELECT 1 FROM close_cleanup_failures failure
             JOIN native_close_completion_retry_admissions admission ON admission.attempt_id = failure.attempt_id
             WHERE failure.failure_occurrence_id = NEW.failure_occurrence_id)
BEGIN
    SELECT RAISE(ABORT, 'automatic completion proof permits no resource-failure children');
END;
