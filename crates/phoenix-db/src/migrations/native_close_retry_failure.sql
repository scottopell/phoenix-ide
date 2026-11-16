CREATE TRIGGER native_close_retry_failure_parent_requires_exact_plan
BEFORE INSERT ON close_cleanup_failures
WHEN NEW.authority_kind = 'expected_resource' AND NEW.cleanup_run_ordinal > 1
 AND EXISTS (SELECT 1 FROM close_obligations obligation
     WHERE obligation.attempt_id = NEW.attempt_id
       AND obligation.product_conversation_id = NEW.source_product_conversation_id
       AND obligation.native_managed = 1)
 AND NOT EXISTS (
    SELECT 1 FROM native_close_retry_plans plan
    JOIN close_runs run ON run.attempt_id = plan.attempt_id AND run.run_ordinal = plan.run_ordinal
    JOIN close_runs prior ON prior.attempt_id = plan.attempt_id AND prior.run_ordinal = plan.failed_run_ordinal
    JOIN native_close_initial_receipts receipt ON receipt.request_id = plan.request_id
     AND receipt.request_fingerprint = plan.request_fingerprint
     AND receipt.attempt_id = plan.attempt_id AND receipt.run_ordinal = plan.run_ordinal
    JOIN close_obligations obligation ON obligation.attempt_id = plan.attempt_id
    JOIN close_original_retirement_anchors anchor ON anchor.attempt_id = plan.attempt_id
     AND anchor.inspection_generation = plan.inspection_generation
     AND anchor.inspection_fingerprint = plan.inspection_fingerprint
    JOIN close_run_retry_effects effect ON effect.attempt_id = plan.attempt_id AND effect.run_ordinal = plan.run_ordinal
    JOIN close_attempt_scopes captured ON captured.attempt_id = effect.attempt_id AND captured.scope = effect.scope
    JOIN close_expected_retirement_resources expected ON expected.attempt_id = effect.attempt_id
     AND expected.scope = effect.scope AND expected.resource_kind = effect.resource_kind
     AND expected.identity_kind = effect.identity_kind AND expected.identity_codec = effect.identity_codec
     AND expected.identity_value = effect.identity_value
     AND expected.inspection_generation = plan.inspection_generation
     AND expected.inspection_fingerprint = plan.inspection_fingerprint
    JOIN close_retirement_inventories inventory ON inventory.attempt_id = expected.attempt_id
     AND inventory.scope = expected.scope AND inventory.inspection_generation = expected.inspection_generation
     AND inventory.inspection_fingerprint = expected.inspection_fingerprint AND inventory.sealed = 1
    WHERE plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.cleanup_run_ordinal
      AND receipt.intent = 'explicit_retry' AND receipt.product_conversation_id = NEW.source_product_conversation_id
      AND obligation.product_conversation_id = NEW.source_product_conversation_id
      AND obligation.native_managed = 1 AND obligation.phase = 'completed'
      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND NOT EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = plan.attempt_id)
      AND run.status = 'running' AND run.retry_evidence_kind = 'resource_plan'
      AND run.run_ordinal > 1 AND plan.failed_run_ordinal = run.run_ordinal - 1 AND prior.status = 'stopped'
      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
      AND plan.evidence_kind = 'resource_plan' AND plan.effect_count > 0
      AND plan.effect_count = (SELECT COUNT(*) FROM close_run_retry_effects listed
          WHERE listed.attempt_id = plan.attempt_id AND listed.run_ordinal = plan.run_ordinal)
      AND NEW.inspection_generation = plan.inspection_generation
      AND NEW.inspection_fingerprint = plan.inspection_fingerprint
      AND effect.scope = NEW.scope AND effect.resource_kind = NEW.resource_kind
      AND effect.identity_kind = NEW.identity_kind AND effect.identity_codec = NEW.identity_codec
      AND effect.identity_value = NEW.identity_value
      AND effect.resource_kind IN ('worktree', 'work_scope', 'tmux_server')
      AND (effect.resource_kind <> 'worktree' OR (effect.identity_kind = 'worktree'
          AND effect.identity_codec = 'worktree_id_v1' AND effect.identity_value = captured.captured_worktree_identity
          AND captured.captured_worktree_fingerprint IS NOT NULL AND captured.captured_worktree_locator IS NOT NULL))
      AND (effect.resource_kind <> 'work_scope' OR (effect.identity_kind = 'opaque'
          AND effect.identity_codec = 'opaque_string_v1' AND effect.identity_value = captured.scope))
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
          WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
            AND success.ordinal = effect.ordinal)
      AND NEW.occurred_at_us >= run.created_at_us
      AND ((NEW.stop_certainty = 'shutdown_uncertain' AND NEW.confirmed_at_us IS NULL)
        OR (NEW.stop_certainty = 'conversation_and_processes_stopped'
          AND NEW.confirmed_at_us >= 0 AND NEW.confirmed_at_us <= NEW.occurred_at_us
          AND NOT EXISTS (
            SELECT 1 FROM close_expected_retirement_resources process
            WHERE process.attempt_id = plan.attempt_id
              AND process.inspection_generation = plan.inspection_generation
              AND process.inspection_fingerprint = plan.inspection_fingerprint
              AND process.resource_kind IN ('bash_process_group', 'tmux_server', 'pty_session', 'browser_session', 'equivalent_live_resource')
              AND NOT EXISTS (SELECT 1 FROM close_retirement_resources retired
                  WHERE retired.attempt_id = process.attempt_id AND retired.scope = process.scope
                    AND retired.inspection_generation = process.inspection_generation
                    AND retired.inspection_fingerprint = process.inspection_fingerprint
                    AND retired.resource_kind = process.resource_kind AND retired.identity_kind = process.identity_kind
                    AND retired.identity_codec = process.identity_codec AND retired.identity_value = process.identity_value
                    AND retired.proof_kind IN ('retired', 'absence_adopted'))
              AND NOT EXISTS (
                SELECT 1 FROM close_run_retry_successes success
                JOIN close_run_retry_effects done ON done.attempt_id = success.attempt_id
                 AND done.run_ordinal = success.run_ordinal AND done.ordinal = success.ordinal
                JOIN native_close_retry_plans done_plan ON done_plan.attempt_id = done.attempt_id
                 AND done_plan.run_ordinal = done.run_ordinal
                JOIN native_close_initial_receipts done_receipt ON done_receipt.request_id = done_plan.request_id
                 AND done_receipt.request_fingerprint = done_plan.request_fingerprint
                 AND done_receipt.attempt_id = done_plan.attempt_id AND done_receipt.run_ordinal = done_plan.run_ordinal
                 AND done_receipt.intent = 'explicit_retry'
                 AND done_receipt.product_conversation_id = receipt.product_conversation_id
                WHERE done.attempt_id = process.attempt_id AND done.run_ordinal <= plan.run_ordinal
                  AND done_plan.inspection_generation = plan.inspection_generation
                  AND done_plan.inspection_fingerprint = plan.inspection_fingerprint
                  AND done.scope = process.scope AND done.resource_kind = process.resource_kind
                  AND done.identity_kind = process.identity_kind AND done.identity_codec = process.identity_codec
                  AND done.identity_value = process.identity_value))))
)
BEGIN
    SELECT RAISE(ABORT, 'native expected-resource retry failure requires its exact anchored plan');
END;

CREATE TRIGGER native_close_retry_failure_child_requires_exact_pending_tail
BEFORE INSERT ON close_cleanup_failure_resources
WHEN EXISTS (SELECT 1 FROM close_cleanup_failures failure
    JOIN close_obligations obligation ON obligation.attempt_id = failure.attempt_id
     AND obligation.product_conversation_id = failure.source_product_conversation_id
    WHERE failure.failure_occurrence_id = NEW.failure_occurrence_id
      AND failure.authority_kind = 'expected_resource' AND failure.cleanup_run_ordinal > 1
      AND obligation.native_managed = 1)
 AND NOT EXISTS (
    SELECT 1 FROM close_cleanup_failures failure
    JOIN close_obligations obligation ON obligation.attempt_id = failure.attempt_id
     AND obligation.product_conversation_id = failure.source_product_conversation_id
    JOIN close_runs run ON run.attempt_id = failure.attempt_id AND run.run_ordinal = failure.cleanup_run_ordinal
    JOIN native_close_retry_plans plan ON plan.attempt_id = run.attempt_id AND plan.run_ordinal = run.run_ordinal
    JOIN close_runs prior ON prior.attempt_id = plan.attempt_id AND prior.run_ordinal = plan.failed_run_ordinal
    JOIN native_close_initial_receipts receipt ON receipt.request_id = plan.request_id
     AND receipt.request_fingerprint = plan.request_fingerprint
     AND receipt.attempt_id = plan.attempt_id AND receipt.run_ordinal = plan.run_ordinal
     AND receipt.intent = 'explicit_retry' AND receipt.product_conversation_id = failure.source_product_conversation_id
    JOIN close_original_retirement_anchors anchor ON anchor.attempt_id = plan.attempt_id
     AND anchor.inspection_generation = plan.inspection_generation
     AND anchor.inspection_fingerprint = plan.inspection_fingerprint
    JOIN close_run_retry_effects effect ON effect.attempt_id = plan.attempt_id AND effect.run_ordinal = plan.run_ordinal
     AND effect.scope = NEW.scope AND effect.resource_kind = NEW.resource_kind
     AND effect.identity_kind = NEW.identity_kind AND effect.identity_codec = NEW.identity_codec
     AND effect.identity_value = NEW.identity_value
    JOIN close_attempt_scopes captured ON captured.attempt_id = effect.attempt_id AND captured.scope = effect.scope
    JOIN close_expected_retirement_resources expected ON expected.attempt_id = effect.attempt_id
     AND expected.scope = effect.scope AND expected.resource_kind = effect.resource_kind
     AND expected.identity_kind = effect.identity_kind AND expected.identity_codec = effect.identity_codec
     AND expected.identity_value = effect.identity_value
     AND expected.inspection_generation = plan.inspection_generation
     AND expected.inspection_fingerprint = plan.inspection_fingerprint
    JOIN close_retirement_inventories inventory ON inventory.attempt_id = expected.attempt_id
     AND inventory.scope = expected.scope AND inventory.inspection_generation = expected.inspection_generation
     AND inventory.inspection_fingerprint = expected.inspection_fingerprint AND inventory.sealed = 1
    WHERE failure.failure_occurrence_id = NEW.failure_occurrence_id
      AND obligation.native_managed = 1 AND obligation.phase = 'completed'
      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND NOT EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = plan.attempt_id)
      AND plan.failed_run_ordinal = run.run_ordinal - 1 AND prior.status = 'stopped'
      AND run.status = 'stopped' AND run.ended_at_us = failure.occurred_at_us
      AND failure.occurred_at_us >= run.created_at_us
      AND failure.inspection_generation = plan.inspection_generation
      AND failure.inspection_fingerprint = plan.inspection_fingerprint
      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
      AND run.retry_evidence_kind = 'resource_plan' AND plan.evidence_kind = 'resource_plan'
      AND plan.effect_count > 0
      AND plan.effect_count = (SELECT COUNT(*) FROM close_run_retry_effects listed
          WHERE listed.attempt_id = plan.attempt_id AND listed.run_ordinal = plan.run_ordinal)
      AND effect.resource_kind IN ('worktree', 'work_scope', 'tmux_server')
      AND (effect.resource_kind <> 'worktree' OR (effect.identity_kind = 'worktree'
          AND effect.identity_codec = 'worktree_id_v1' AND effect.identity_value = captured.captured_worktree_identity
          AND captured.captured_worktree_fingerprint IS NOT NULL AND captured.captured_worktree_locator IS NOT NULL))
      AND (effect.resource_kind <> 'work_scope' OR (effect.identity_kind = 'opaque'
          AND effect.identity_codec = 'opaque_string_v1' AND effect.identity_value = captured.scope))
      AND NOT EXISTS (SELECT 1 FROM coordinator_watch_events WHERE event_id = NEW.failure_occurrence_id)
      AND NEW.ordinal = (SELECT COUNT(*) FROM close_cleanup_failure_resources
          WHERE failure_occurrence_id = NEW.failure_occurrence_id)
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
          WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
            AND success.ordinal = effect.ordinal)
      AND NEW.ordinal = (SELECT COUNT(*) FROM close_run_retry_effects earlier
          WHERE earlier.attempt_id = effect.attempt_id AND earlier.run_ordinal = effect.run_ordinal
            AND earlier.ordinal < effect.ordinal
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
                WHERE success.attempt_id = earlier.attempt_id AND success.run_ordinal = earlier.run_ordinal
                  AND success.ordinal = earlier.ordinal))
      AND NEW.disposition = CASE WHEN effect.scope = failure.scope AND effect.resource_kind = failure.resource_kind
          AND effect.identity_kind = failure.identity_kind AND effect.identity_codec = failure.identity_codec
          AND effect.identity_value = failure.identity_value THEN 'failed' ELSE 'unattempted' END
)
BEGIN
    SELECT RAISE(ABORT, 'native expected-resource retry children require exact ordered pending effects');
END;
