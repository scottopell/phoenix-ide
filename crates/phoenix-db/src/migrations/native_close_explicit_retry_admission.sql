-- One receipt authority for initial, observation, and exact retry.
CREATE TABLE native_close_receipts_v161 (
    request_id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(request_id)) > 0),
    source_transcript_id TEXT NOT NULL CHECK (length(trim(source_transcript_id)) > 0),
    source_message_id TEXT NOT NULL CHECK (length(trim(source_message_id)) > 0),
    source_tool_use_id TEXT NOT NULL CHECK (length(trim(source_tool_use_id)) > 0),
    request_fingerprint TEXT NOT NULL CHECK (length(trim(request_fingerprint)) > 0),
    product_conversation_id TEXT NOT NULL CHECK (length(trim(product_conversation_id)) > 0),
    attempt_id TEXT NOT NULL CHECK (length(trim(attempt_id)) > 0),
    run_ordinal INTEGER NOT NULL CHECK (typeof(run_ordinal) = 'integer' AND run_ordinal > 0),
    intent TEXT NOT NULL DEFAULT 'initial' CHECK (intent IN ('initial', 'observe', 'explicit_retry')),
    CHECK (intent <> 'initial' OR run_ordinal = 1),
    CHECK (intent <> 'explicit_retry' OR run_ordinal > 1),
    UNIQUE (source_transcript_id, source_message_id, source_tool_use_id),
    UNIQUE (request_id, request_fingerprint, attempt_id, run_ordinal),
    FOREIGN KEY (attempt_id, product_conversation_id)
        REFERENCES close_obligations(attempt_id, product_conversation_id) ON DELETE CASCADE,
    FOREIGN KEY (attempt_id, run_ordinal)
        REFERENCES close_runs(attempt_id, run_ordinal) ON DELETE CASCADE
);
INSERT INTO native_close_receipts_v161 SELECT * FROM native_close_initial_receipts;
DROP TABLE native_close_initial_receipts;
ALTER TABLE native_close_receipts_v161 RENAME TO native_close_initial_receipts;

CREATE TRIGGER native_close_initial_receipts_require_native_parent
BEFORE INSERT ON native_close_initial_receipts
WHEN NOT EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = NEW.attempt_id
    AND product_conversation_id = NEW.product_conversation_id AND native_managed = 1)
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
      AND source_message_id = NEW.source_message_id AND source_tool_use_id = NEW.source_tool_use_id)
BEGIN
    SELECT RAISE(ABORT, 'native Close receipt cannot be replaced');
END;
CREATE TRIGGER native_close_initial_receipts_no_direct_delete
BEFORE DELETE ON native_close_initial_receipts
WHEN EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'native Close receipt must be retained with its obligation');
END;

CREATE TABLE native_close_retry_plans (
    attempt_id TEXT NOT NULL,
    failed_run_ordinal INTEGER NOT NULL CHECK (typeof(failed_run_ordinal) = 'integer' AND failed_run_ordinal > 0),
    run_ordinal INTEGER NOT NULL CHECK (typeof(run_ordinal) = 'integer' AND run_ordinal = failed_run_ordinal + 1),
    request_id TEXT NOT NULL UNIQUE,
    request_fingerprint TEXT NOT NULL CHECK (length(trim(request_fingerprint)) > 0),
    inspection_generation TEXT NOT NULL,
    inspection_fingerprint TEXT NOT NULL,
    plan_fingerprint TEXT NOT NULL CHECK (length(trim(plan_fingerprint)) > 0),
    evidence_kind TEXT NOT NULL CHECK (evidence_kind = 'resource_plan'),
    effect_count INTEGER NOT NULL CHECK (typeof(effect_count) = 'integer' AND effect_count > 0),
    PRIMARY KEY (attempt_id, run_ordinal),
    FOREIGN KEY (attempt_id, failed_run_ordinal) REFERENCES close_runs(attempt_id, run_ordinal),
    FOREIGN KEY (attempt_id, run_ordinal) REFERENCES close_runs(attempt_id, run_ordinal)
        ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (request_id, request_fingerprint, attempt_id, run_ordinal)
        REFERENCES native_close_initial_receipts(request_id, request_fingerprint, attempt_id, run_ordinal)
        DEFERRABLE INITIALLY DEFERRED
);
CREATE TRIGGER native_close_retry_plans_require_original
BEFORE INSERT ON native_close_retry_plans
WHEN EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id
    AND run_ordinal = NEW.run_ordinal OR request_id = NEW.request_id)
  OR EXISTS (SELECT 1 FROM close_runs WHERE attempt_id = NEW.attempt_id AND run_ordinal >= NEW.run_ordinal)
  OR NOT EXISTS (SELECT 1 FROM close_original_retirement_anchors anchor
    JOIN close_runs prior ON prior.attempt_id = anchor.attempt_id
    JOIN close_obligations obligation ON obligation.attempt_id = prior.attempt_id
    WHERE prior.attempt_id = NEW.attempt_id AND prior.run_ordinal = NEW.failed_run_ordinal
      AND prior.status = 'stopped' AND obligation.phase = 'completed' AND obligation.native_managed = 1
      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND anchor.inspection_generation = NEW.inspection_generation
      AND anchor.inspection_fingerprint = NEW.inspection_fingerprint)
  OR EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = NEW.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'native retry header requires exact stopped original authority');
END;
CREATE TRIGGER native_close_retry_plans_immutable
BEFORE UPDATE ON native_close_retry_plans
BEGIN
    SELECT RAISE(ABORT, 'native retry header is immutable');
END;
CREATE TRIGGER native_close_retry_plans_no_direct_delete
BEFORE DELETE ON native_close_retry_plans
WHEN EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'native retry header must be retained');
END;
CREATE TRIGGER native_close_retry_receipts_require_plan
BEFORE INSERT ON native_close_initial_receipts
WHEN NEW.intent = 'explicit_retry' AND NOT EXISTS (
    SELECT 1 FROM native_close_retry_plans plan WHERE plan.request_id = NEW.request_id
      AND plan.request_fingerprint = NEW.request_fingerprint
      AND plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.run_ordinal)
  OR NEW.intent <> 'explicit_retry' AND EXISTS (
    SELECT 1 FROM native_close_retry_plans plan WHERE plan.request_id = NEW.request_id)
BEGIN
    SELECT RAISE(ABORT, 'native retry receipt requires its exact plan');
END;

CREATE TRIGGER native_close_retry_effects_no_direct_delete
BEFORE DELETE ON close_run_retry_effects
WHEN EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = OLD.attempt_id AND run_ordinal = OLD.run_ordinal)
  AND EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'native retry effects must be retained');
END;

DROP TRIGGER close_run_retry_effects_require_remaining_authority;
CREATE TRIGGER close_run_retry_effects_require_remaining_authority
BEFORE INSERT ON close_run_retry_effects
WHEN EXISTS (SELECT 1 FROM close_runs WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
  OR (EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
    AND EXISTS (SELECT 1 FROM close_run_retry_effects WHERE attempt_id = NEW.attempt_id
      AND run_ordinal = NEW.run_ordinal AND ordinal = NEW.ordinal))
  OR (NOT EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
    AND (NOT EXISTS (
      SELECT 1 FROM close_runs prior JOIN close_obligations obligation ON obligation.attempt_id = prior.attempt_id
      JOIN close_cleanup_failures failure ON failure.attempt_id = prior.attempt_id AND failure.cleanup_run_ordinal = prior.run_ordinal
      JOIN close_cleanup_failure_resources remaining ON remaining.failure_occurrence_id = failure.failure_occurrence_id
        AND remaining.ordinal = NEW.ordinal AND remaining.scope = NEW.scope
        AND remaining.resource_kind = NEW.resource_kind AND remaining.identity_kind = NEW.identity_kind
        AND remaining.identity_codec = NEW.identity_codec AND remaining.identity_value = NEW.identity_value
      JOIN close_expected_retirement_resources expected ON expected.attempt_id = prior.attempt_id
      JOIN close_retirement_inventories inventory ON inventory.attempt_id = expected.attempt_id
        AND inventory.scope = expected.scope AND inventory.inspection_generation = expected.inspection_generation
        AND inventory.inspection_fingerprint = expected.inspection_fingerprint AND inventory.sealed = 1
      WHERE prior.attempt_id = NEW.attempt_id AND prior.run_ordinal = NEW.run_ordinal - 1 AND prior.status = 'stopped'
        AND NEW.resource_kind NOT IN ('bash_process_group', 'pty_session', 'browser_session', 'equivalent_live_resource')
        AND expected.scope = NEW.scope AND expected.resource_kind = NEW.resource_kind
        AND expected.identity_kind = NEW.identity_kind AND expected.identity_codec = NEW.identity_codec
        AND expected.identity_value = NEW.identity_value
        AND expected.inspection_generation = obligation.inspection_generation
        AND expected.inspection_fingerprint = obligation.inspection_fingerprint
  )
  OR EXISTS (
      SELECT 1 FROM close_retirement_resources retired WHERE retired.attempt_id = NEW.attempt_id
        AND retired.scope = NEW.scope AND retired.resource_kind = NEW.resource_kind
        AND retired.identity_kind = NEW.identity_kind AND retired.identity_codec = NEW.identity_codec
        AND retired.identity_value = NEW.identity_value AND retired.proof_kind <> 'residual'
  )))
  OR (EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
    AND NOT EXISTS (
      SELECT 1 FROM native_close_retry_plans plan
      JOIN close_runs prior ON prior.attempt_id = plan.attempt_id AND prior.run_ordinal = plan.failed_run_ordinal AND prior.status = 'stopped'
      JOIN close_retirement_inventories inventory ON inventory.attempt_id = plan.attempt_id
        AND inventory.scope = NEW.scope AND inventory.inspection_generation = plan.inspection_generation
        AND inventory.inspection_fingerprint = plan.inspection_fingerprint AND inventory.sealed = 1
      JOIN close_expected_retirement_resources expected ON expected.attempt_id = inventory.attempt_id
        AND expected.scope = inventory.scope AND expected.inspection_generation = inventory.inspection_generation
        AND expected.inspection_fingerprint = inventory.inspection_fingerprint
      JOIN close_retirement_resources residual ON residual.attempt_id = expected.attempt_id
        AND residual.scope = expected.scope AND residual.inspection_generation = expected.inspection_generation
        AND residual.inspection_fingerprint = expected.inspection_fingerprint
        AND residual.resource_kind = expected.resource_kind AND residual.identity_kind = expected.identity_kind
        AND residual.identity_codec = expected.identity_codec AND residual.identity_value = expected.identity_value
        AND residual.proof_kind = 'residual'
      WHERE plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.run_ordinal
        AND plan.evidence_kind = 'resource_plan' AND NEW.ordinal < plan.effect_count
        AND NEW.resource_kind NOT IN ('bash_process_group', 'pty_session', 'browser_session', 'equivalent_live_resource')
        AND expected.resource_kind = NEW.resource_kind AND expected.identity_kind = NEW.identity_kind
        AND expected.identity_codec = NEW.identity_codec AND expected.identity_value = NEW.identity_value
        AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
          JOIN close_run_retry_effects done ON done.attempt_id = success.attempt_id
            AND done.run_ordinal = success.run_ordinal AND done.ordinal = success.ordinal
          JOIN native_close_retry_plans done_plan ON done_plan.attempt_id = done.attempt_id
            AND done_plan.run_ordinal = done.run_ordinal
          JOIN native_close_initial_receipts done_receipt ON done_receipt.request_id = done_plan.request_id
            AND done_receipt.request_fingerprint = done_plan.request_fingerprint
            AND done_receipt.attempt_id = done_plan.attempt_id AND done_receipt.run_ordinal = done_plan.run_ordinal
            AND done_receipt.intent = 'explicit_retry'
          WHERE success.attempt_id = plan.attempt_id AND success.run_ordinal <= plan.failed_run_ordinal
            AND done_plan.inspection_generation = plan.inspection_generation
            AND done_plan.inspection_fingerprint = plan.inspection_fingerprint
            AND done.scope = NEW.scope AND done.resource_kind = NEW.resource_kind
            AND done.identity_kind = NEW.identity_kind AND done.identity_codec = NEW.identity_codec
            AND done.identity_value = NEW.identity_value)
    ))
BEGIN
    SELECT RAISE(ABORT, 'safe retry effects require exact remaining original authority before run admission');
END;

DROP TRIGGER close_runs_require_next_ordinal;
CREATE TRIGGER close_runs_require_next_ordinal
BEFORE INSERT ON close_runs
WHEN NEW.status <> 'running'
  OR NEW.run_ordinal <> COALESCE((SELECT MAX(run_ordinal) + 1 FROM close_runs WHERE attempt_id = NEW.attempt_id), 1)
  OR (NEW.run_ordinal > 1
    AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
    AND NOT EXISTS (
      SELECT 1 FROM close_runs prior JOIN close_cleanup_failures failure
        ON failure.attempt_id = prior.attempt_id AND failure.cleanup_run_ordinal = prior.run_ordinal
      JOIN close_obligations obligation ON obligation.attempt_id = prior.attempt_id
      WHERE prior.attempt_id = NEW.attempt_id AND prior.run_ordinal = NEW.run_ordinal - 1
        AND prior.status = 'stopped' AND NEW.retry_observed_at_us > failure.occurred_at_us
        AND NEW.retry_observed_at_us <= NEW.created_at_us
        AND obligation.phase = 'completed'
        AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
        AND ((NEW.retry_evidence_kind = 'resource_plan'
          AND EXISTS (SELECT 1 FROM close_run_retry_effects effect
            WHERE effect.attempt_id = NEW.attempt_id AND effect.run_ordinal = NEW.run_ordinal)
          AND (SELECT COUNT(*) FROM close_run_retry_effects effect
            WHERE effect.attempt_id = NEW.attempt_id AND effect.run_ordinal = NEW.run_ordinal)
            = (SELECT COUNT(*) FROM close_cleanup_failure_resources resource
               WHERE resource.failure_occurrence_id = failure.failure_occurrence_id))
          OR (NEW.retry_evidence_kind = 'verified_completion' AND prior.run_ordinal > 1
            AND prior.retry_evidence_kind IN ('resource_plan', 'verified_completion')
            AND failure.authority_kind = 'attempt_interrupted'
            AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
              WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
              WHERE effect.attempt_id = NEW.attempt_id AND effect.run_ordinal = NEW.run_ordinal)
            AND ((prior.retry_evidence_kind = 'resource_plan'
              AND EXISTS (SELECT 1 FROM close_run_retry_effects effect
                WHERE effect.attempt_id = prior.attempt_id AND effect.run_ordinal = prior.run_ordinal)
              AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
                WHERE effect.attempt_id = prior.attempt_id AND effect.run_ordinal = prior.run_ordinal
                  AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
                    WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal AND success.ordinal = effect.ordinal)))
            OR (prior.retry_evidence_kind = 'verified_completion'
              AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
                WHERE effect.attempt_id = prior.attempt_id AND effect.run_ordinal = prior.run_ordinal))))
  )))
  OR (EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
    AND NOT EXISTS (
      SELECT 1 FROM native_close_retry_plans plan
      JOIN close_runs prior ON prior.attempt_id = plan.attempt_id AND prior.run_ordinal = plan.failed_run_ordinal
      JOIN close_cleanup_failures failure ON failure.attempt_id = prior.attempt_id AND failure.cleanup_run_ordinal = prior.run_ordinal
      WHERE plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.run_ordinal
        AND prior.status = 'stopped' AND NEW.retry_requested_by = 'user'
        AND NEW.retry_observed_at_us > failure.occurred_at_us AND NEW.retry_observed_at_us <= NEW.created_at_us
        AND NEW.retry_evidence_kind = plan.evidence_kind
        AND plan.effect_count = (SELECT COUNT(*) FROM close_run_retry_effects effect
          WHERE effect.attempt_id = NEW.attempt_id AND effect.run_ordinal = NEW.run_ordinal)
        AND plan.evidence_kind = 'resource_plan'
    ))
BEGIN
    SELECT RAISE(ABORT, 'Close run requires initial or fresh next stopped-run authority');
END;
