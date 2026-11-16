CREATE TRIGGER native_close_source_fence_requires_exact_plan
BEFORE INSERT ON close_cleanup_failures
WHEN NEW.authority_kind = 'explicitly_suppressed' AND NOT EXISTS (
    SELECT 1 FROM native_close_retry_plans plan
    JOIN native_close_initial_receipts receipt
      ON receipt.request_id = plan.request_id
     AND receipt.request_fingerprint = plan.request_fingerprint
     AND receipt.attempt_id = plan.attempt_id AND receipt.run_ordinal = plan.run_ordinal
    JOIN close_runs run ON run.attempt_id = plan.attempt_id AND run.run_ordinal = plan.run_ordinal
    JOIN close_runs prior ON prior.attempt_id = plan.attempt_id AND prior.run_ordinal = plan.failed_run_ordinal
    JOIN close_obligations obligation ON obligation.attempt_id = plan.attempt_id
    JOIN close_original_retirement_anchors anchor ON anchor.attempt_id = plan.attempt_id
     AND anchor.inspection_generation = plan.inspection_generation
     AND anchor.inspection_fingerprint = plan.inspection_fingerprint
    WHERE plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.cleanup_run_ordinal
      AND receipt.intent = 'explicit_retry'
      AND receipt.product_conversation_id = NEW.source_product_conversation_id
      AND obligation.product_conversation_id = NEW.source_product_conversation_id
      AND obligation.native_managed = 1 AND obligation.phase = 'completed'
      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND run.status = 'running' AND run.retry_evidence_kind = 'resource_plan'
      AND run.run_ordinal > 1 AND prior.status = 'stopped'
      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
      AND plan.evidence_kind = 'resource_plan'
      AND plan.effect_count = (SELECT COUNT(*) FROM close_run_retry_effects effect
          WHERE effect.attempt_id = plan.attempt_id AND effect.run_ordinal = plan.run_ordinal)
      AND NEW.stop_certainty = 'shutdown_uncertain' AND NEW.confirmed_at_us IS NULL
      AND NEW.reason = 'interrupted' AND NEW.occurred_at_us >= run.created_at_us
      AND NEW.scope IS NULL AND NEW.resource_kind IS NULL AND NEW.identity_kind IS NULL
      AND NEW.identity_codec IS NULL AND NEW.identity_value IS NULL
      AND NEW.inspection_generation IS NULL AND NEW.inspection_fingerprint IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'source fence requires exact latest running native retry plan and receipt');
END;

CREATE TRIGGER native_close_source_fence_children_require_pending_plan
BEFORE INSERT ON close_cleanup_failure_resources
WHEN EXISTS (SELECT 1 FROM close_cleanup_failures failure
    WHERE failure.failure_occurrence_id = NEW.failure_occurrence_id
      AND failure.authority_kind = 'explicitly_suppressed')
 AND NOT EXISTS (
    SELECT 1 FROM close_cleanup_failures failure
    JOIN close_runs run ON run.attempt_id = failure.attempt_id AND run.run_ordinal = failure.cleanup_run_ordinal
    JOIN native_close_retry_plans plan ON plan.attempt_id = run.attempt_id AND plan.run_ordinal = run.run_ordinal
    JOIN native_close_initial_receipts receipt ON receipt.request_id = plan.request_id
     AND receipt.request_fingerprint = plan.request_fingerprint
     AND receipt.attempt_id = plan.attempt_id AND receipt.run_ordinal = plan.run_ordinal
     AND receipt.product_conversation_id = failure.source_product_conversation_id
     AND receipt.intent = 'explicit_retry'
    JOIN close_run_retry_effects effect ON effect.attempt_id = plan.attempt_id AND effect.run_ordinal = plan.run_ordinal
    WHERE failure.failure_occurrence_id = NEW.failure_occurrence_id
      AND run.status = 'stopped' AND run.ended_at_us = failure.occurred_at_us
      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
      AND NEW.disposition = 'unattempted'
      AND NOT EXISTS (SELECT 1 FROM coordinator_watch_events WHERE event_id = NEW.failure_occurrence_id)
      AND NEW.ordinal = (SELECT COUNT(*) FROM close_cleanup_failure_resources
          WHERE failure_occurrence_id = NEW.failure_occurrence_id)
      AND effect.scope = NEW.scope AND effect.resource_kind = NEW.resource_kind
      AND effect.identity_kind = NEW.identity_kind AND effect.identity_codec = NEW.identity_codec
      AND effect.identity_value = NEW.identity_value
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
          WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
            AND success.ordinal = effect.ordinal)
      AND NEW.ordinal = (SELECT COUNT(*) FROM close_run_retry_effects earlier
          WHERE earlier.attempt_id = effect.attempt_id AND earlier.run_ordinal = effect.run_ordinal
            AND earlier.ordinal < effect.ordinal
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
                WHERE success.attempt_id = earlier.attempt_id AND success.run_ordinal = earlier.run_ordinal
                  AND success.ordinal = earlier.ordinal))
)
BEGIN
    SELECT RAISE(ABORT, 'source fence children must be exact ordered pending unattempted effects');
END;
