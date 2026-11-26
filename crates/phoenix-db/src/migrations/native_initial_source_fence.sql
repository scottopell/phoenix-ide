DROP TRIGGER native_close_source_fence_requires_exact_plan;
CREATE TRIGGER native_close_source_fence_requires_exact_plan
BEFORE INSERT ON close_cleanup_failures
WHEN (NEW.authority_kind = 'explicitly_suppressed'
    OR (NEW.authority_kind = 'attempt_interrupted' AND (
        EXISTS (SELECT 1 FROM native_close_retry_plans plan
                WHERE plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.cleanup_run_ordinal)
        OR EXISTS (SELECT 1 FROM native_close_initial_receipts receipt
                   WHERE receipt.attempt_id = NEW.attempt_id
                     AND receipt.run_ordinal = NEW.cleanup_run_ordinal AND receipt.intent = 'initial'))))
 AND NOT (
    EXISTS (
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
    OR EXISTS (
        SELECT 1 FROM native_close_initial_receipts receipt
        JOIN close_runs run ON run.attempt_id = receipt.attempt_id AND run.run_ordinal = receipt.run_ordinal
        JOIN close_obligations obligation ON obligation.attempt_id = receipt.attempt_id
        JOIN product_conversations product ON product.id = obligation.product_conversation_id
        WHERE receipt.attempt_id = NEW.attempt_id AND receipt.run_ordinal = NEW.cleanup_run_ordinal
          AND receipt.intent = 'initial' AND receipt.request_fingerprint <> ''
          AND receipt.product_conversation_id = NEW.source_product_conversation_id
          AND obligation.product_conversation_id = NEW.source_product_conversation_id
          AND obligation.native_managed = 1 AND obligation.phase IN
            ('awaiting_blocker_resolution', 'awaiting_stop_work_confirmation', 'settling_active_work', 'awaiting_retirement_inspection')
          AND obligation.inspection_generation IS NULL AND obligation.inspection_fingerprint IS NULL
          AND obligation.topology_sealed = 1
          AND product.kind = 'ordinary' AND product.ordinary_lifecycle = 'open'
          AND run.run_ordinal = 1 AND run.status = 'running'
          AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
          AND NEW.stop_certainty = 'shutdown_uncertain' AND NEW.confirmed_at_us IS NULL
          AND NEW.reason = 'interrupted' AND NEW.occurred_at_us >= run.created_at_us AND NEW.scope IS NULL AND NEW.resource_kind IS NULL
          AND NEW.identity_kind IS NULL AND NEW.identity_codec IS NULL AND NEW.identity_value IS NULL
          AND NEW.inspection_generation IS NULL AND NEW.inspection_fingerprint IS NULL
          AND NOT EXISTS (SELECT 1 FROM close_retirement_inspections WHERE attempt_id = receipt.attempt_id)
          AND NOT EXISTS (SELECT 1 FROM close_retirement_inventories WHERE attempt_id = receipt.attempt_id)
          AND NOT EXISTS (SELECT 1 FROM close_retirement_losses WHERE attempt_id = receipt.attempt_id)
          AND NOT EXISTS (SELECT 1 FROM close_expected_retirement_resources WHERE attempt_id = receipt.attempt_id)
          AND NOT EXISTS (SELECT 1 FROM close_retirement_resources WHERE attempt_id = receipt.attempt_id)
          AND NOT EXISTS (SELECT 1 FROM close_retirement_resource_dispatches WHERE attempt_id = receipt.attempt_id)
          AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects WHERE attempt_id = receipt.attempt_id)
          AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes WHERE attempt_id = receipt.attempt_id)
          AND NOT EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = receipt.attempt_id)
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'source fence requires an exact native retry or effect-free initial receipt');
END;

CREATE INDEX native_close_initial_source_fence_lookup
    ON native_close_initial_receipts(source_transcript_id, intent, attempt_id, run_ordinal);
