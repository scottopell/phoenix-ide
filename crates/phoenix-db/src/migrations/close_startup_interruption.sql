DROP TRIGGER close_cleanup_failures_require_initial_authority;
CREATE TRIGGER close_cleanup_failures_require_initial_authority
BEFORE INSERT ON close_cleanup_failures
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN product_conversations product ON product.id = obligation.product_conversation_id
    LEFT JOIN close_attempt_scopes captured ON captured.attempt_id = obligation.attempt_id AND captured.scope = NEW.scope
    WHERE obligation.attempt_id = NEW.attempt_id AND obligation.phase <> 'completed'
      AND NEW.cleanup_run_ordinal = 1
      AND (captured.scope IS NOT NULL OR (NEW.authority_kind = 'attempt_interrupted'
          AND NEW.stop_certainty = 'shutdown_uncertain'
          AND NOT EXISTS (SELECT 1 FROM close_attempt_scopes WHERE attempt_id = NEW.attempt_id)))
      AND EXISTS (SELECT 1 FROM close_runs run WHERE run.attempt_id = NEW.attempt_id
          AND run.run_ordinal = NEW.cleanup_run_ordinal AND run.status = 'running')
      AND obligation.product_conversation_id = NEW.source_product_conversation_id
      AND product.kind = 'ordinary' AND product.ordinary_lifecycle = 'open'
      AND (
        (NEW.authority_kind = 'expected_resource'
          AND obligation.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'needs_repair')
          AND obligation.inspection_generation = NEW.inspection_generation
          AND obligation.inspection_fingerprint = NEW.inspection_fingerprint
          AND EXISTS (
            SELECT 1 FROM close_retirement_resources residual
            JOIN close_expected_retirement_resources expected
              ON expected.attempt_id = residual.attempt_id AND expected.scope = residual.scope
             AND expected.inspection_generation = residual.inspection_generation
             AND expected.inspection_fingerprint = residual.inspection_fingerprint
             AND expected.resource_kind = residual.resource_kind AND expected.identity_kind = residual.identity_kind
             AND expected.identity_codec = residual.identity_codec AND expected.identity_value = residual.identity_value
            JOIN close_retirement_inventories inventory
              ON inventory.attempt_id = expected.attempt_id AND inventory.scope = expected.scope
             AND inventory.inspection_generation = expected.inspection_generation
             AND inventory.inspection_fingerprint = expected.inspection_fingerprint
            WHERE residual.attempt_id = NEW.attempt_id AND residual.scope = NEW.scope
              AND residual.inspection_generation = NEW.inspection_generation
              AND residual.inspection_fingerprint = NEW.inspection_fingerprint
              AND residual.resource_kind = NEW.resource_kind AND residual.identity_kind = NEW.identity_kind
              AND residual.identity_codec = NEW.identity_codec AND residual.identity_value = NEW.identity_value
              AND residual.proof_kind = 'residual'
              AND (NEW.reason = 'interrupted' OR (residual.residual_reason = NEW.reason AND residual.detail = NEW.detail))
              AND inventory.sealed = 1
          ))
        OR (NEW.authority_kind = 'captured_scope'
          AND ((NEW.resource_kind = 'work_scope' AND NEW.identity_kind = 'opaque'
                AND NEW.identity_codec = 'opaque_string_v1' AND NEW.identity_value = captured.scope)
            OR (NEW.resource_kind = 'worktree' AND NEW.identity_kind = 'worktree'
                AND NEW.identity_codec = 'worktree_id_v1'
                AND NEW.identity_value = captured.captured_worktree_identity
                AND captured.captured_worktree_fingerprint IS NOT NULL
                AND captured.captured_worktree_locator IS NOT NULL)))
        OR (NEW.authority_kind IN ('observed_process_resource', 'attempt_interrupted'))
      )
      AND (NEW.stop_certainty = 'shutdown_uncertain' OR NOT EXISTS (
          SELECT 1 FROM close_expected_retirement_resources expected
          LEFT JOIN close_retirement_resources retired
            ON retired.attempt_id = expected.attempt_id AND retired.scope = expected.scope
           AND retired.inspection_generation = expected.inspection_generation
           AND retired.inspection_fingerprint = expected.inspection_fingerprint
           AND retired.resource_kind = expected.resource_kind AND retired.identity_kind = expected.identity_kind
           AND retired.identity_codec = expected.identity_codec AND retired.identity_value = expected.identity_value
          WHERE expected.attempt_id = NEW.attempt_id
            AND expected.resource_kind IN ('bash_process_group', 'tmux_server', 'pty_session', 'browser_session', 'equivalent_live_resource')
            AND (retired.proof_kind IS NULL OR retired.proof_kind = 'residual')
      ))
) AND NOT EXISTS (
    SELECT 1 FROM close_runs run
    JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
    JOIN close_run_retry_effects effect ON effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal
    JOIN close_expected_retirement_resources expected
      ON expected.attempt_id = effect.attempt_id AND expected.scope = effect.scope
     AND expected.resource_kind = effect.resource_kind AND expected.identity_kind = effect.identity_kind
     AND expected.identity_codec = effect.identity_codec AND expected.identity_value = effect.identity_value
     AND expected.inspection_generation = NEW.inspection_generation AND expected.inspection_fingerprint = NEW.inspection_fingerprint
    JOIN close_retirement_inventories inventory
      ON inventory.attempt_id = expected.attempt_id AND inventory.scope = expected.scope
     AND inventory.inspection_generation = expected.inspection_generation AND inventory.inspection_fingerprint = expected.inspection_fingerprint
     AND inventory.sealed = 1
    WHERE run.attempt_id = NEW.attempt_id AND run.run_ordinal = NEW.cleanup_run_ordinal
      AND run.run_ordinal > 1 AND run.status = 'running'
      AND NEW.authority_kind = 'expected_resource'
      AND obligation.product_conversation_id = NEW.source_product_conversation_id
      AND obligation.phase = 'completed' AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND effect.scope = NEW.scope AND effect.resource_kind = NEW.resource_kind
      AND effect.identity_kind = NEW.identity_kind AND effect.identity_codec = NEW.identity_codec
      AND effect.identity_value = NEW.identity_value
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
          WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
            AND success.ordinal = effect.ordinal)
      AND NEW.inspection_generation = obligation.inspection_generation
      AND NEW.inspection_fingerprint = obligation.inspection_fingerprint
      AND (NEW.stop_certainty = 'shutdown_uncertain' OR NOT EXISTS (
          SELECT 1 FROM close_expected_retirement_resources expected
          LEFT JOIN close_retirement_resources retired
            ON retired.attempt_id = expected.attempt_id AND retired.scope = expected.scope
           AND retired.inspection_generation = expected.inspection_generation
           AND retired.inspection_fingerprint = expected.inspection_fingerprint
           AND retired.resource_kind = expected.resource_kind AND retired.identity_kind = expected.identity_kind
           AND retired.identity_codec = expected.identity_codec AND retired.identity_value = expected.identity_value
          WHERE expected.attempt_id = NEW.attempt_id
            AND expected.resource_kind IN ('bash_process_group', 'tmux_server', 'pty_session', 'browser_session', 'equivalent_live_resource')
            AND (retired.proof_kind IS NULL OR retired.proof_kind = 'residual')
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects prior
                JOIN close_run_retry_successes success ON success.attempt_id = prior.attempt_id
                  AND success.run_ordinal = prior.run_ordinal AND success.ordinal = prior.ordinal
                WHERE prior.attempt_id = NEW.attempt_id
                  AND prior.scope = expected.scope AND prior.resource_kind = expected.resource_kind
                  AND prior.identity_kind = expected.identity_kind AND prior.identity_codec = expected.identity_codec
                  AND prior.identity_value = expected.identity_value)
      ))
) AND NOT EXISTS (
    SELECT 1 FROM close_runs run
    JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
    WHERE run.attempt_id = NEW.attempt_id AND run.run_ordinal = NEW.cleanup_run_ordinal
      AND run.run_ordinal > 1 AND run.status = 'running'
      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
      AND obligation.product_conversation_id = NEW.source_product_conversation_id
      AND obligation.phase = 'completed' AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND NEW.authority_kind = 'attempt_interrupted'
      AND ((run.retry_evidence_kind = 'resource_plan'
        AND EXISTS (SELECT 1 FROM close_run_retry_effects effect
          WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal)
        AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
          WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
              WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
                AND success.ordinal = effect.ordinal)))
      OR (run.retry_evidence_kind = 'verified_completion'
        AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects effect
          WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal)))
      AND (NEW.stop_certainty = 'shutdown_uncertain' OR NOT EXISTS (
          SELECT 1 FROM close_expected_retirement_resources expected
          LEFT JOIN close_retirement_resources retired
            ON retired.attempt_id = expected.attempt_id AND retired.scope = expected.scope
           AND retired.inspection_generation = expected.inspection_generation
           AND retired.inspection_fingerprint = expected.inspection_fingerprint
           AND retired.resource_kind = expected.resource_kind AND retired.identity_kind = expected.identity_kind
           AND retired.identity_codec = expected.identity_codec AND retired.identity_value = expected.identity_value
          WHERE expected.attempt_id = NEW.attempt_id
            AND expected.resource_kind IN ('bash_process_group', 'tmux_server', 'pty_session', 'browser_session', 'equivalent_live_resource')
            AND (retired.proof_kind IS NULL OR retired.proof_kind = 'residual')
            AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects prior
                JOIN close_run_retry_successes success ON success.attempt_id = prior.attempt_id
                  AND success.run_ordinal = prior.run_ordinal AND success.ordinal = prior.ordinal
                WHERE prior.attempt_id = NEW.attempt_id
                  AND prior.scope = expected.scope AND prior.resource_kind = expected.resource_kind
                  AND prior.identity_kind = expected.identity_kind AND prior.identity_codec = expected.identity_codec
                  AND prior.identity_value = expected.identity_value)
      ))
)
BEGIN
    SELECT RAISE(ABORT, 'cleanup failure requires exact active expected-resource, captured-scope, or observed-process authority');
END;
DROP TRIGGER close_cleanup_failure_completion_requires_disposition;
CREATE TRIGGER close_cleanup_failure_completion_requires_disposition
BEFORE UPDATE OF phase, close_outcome ON close_obligations
WHEN NEW.phase = 'completed' AND NEW.close_outcome IN ('archived_cleanup_attention', 'close_incomplete')
AND NOT (OLD.phase = 'completed' AND OLD.close_outcome = 'close_incomplete'
    AND NEW.close_outcome = 'archived_cleanup_attention'
    AND EXISTS (SELECT 1 FROM close_runs run JOIN close_cleanup_failures failure
      ON failure.attempt_id = run.attempt_id AND failure.cleanup_run_ordinal = run.run_ordinal
      WHERE run.attempt_id = NEW.attempt_id AND run.run_ordinal > 1 AND run.status = 'stopped'
        AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = NEW.attempt_id)
        AND failure.source_product_conversation_id = NEW.product_conversation_id
        AND failure.stop_certainty = 'conversation_and_processes_stopped'
        AND failure.confirmed_at_us IS NOT NULL
        AND failure.occurred_at_us = run.ended_at_us))
AND (OLD.phase = 'completed' OR NOT EXISTS (
    SELECT 1 FROM close_cleanup_failures failure
    JOIN product_conversations product ON product.id = failure.source_product_conversation_id
    WHERE failure.attempt_id = NEW.attempt_id AND failure.cleanup_run_ordinal = 1
      AND failure.source_product_conversation_id = NEW.product_conversation_id
      AND ((failure.inspection_generation IS NOT NULL AND failure.inspection_fingerprint IS NOT NULL
            AND OLD.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'needs_repair')
            AND failure.inspection_generation = NEW.inspection_generation
            AND failure.inspection_fingerprint = NEW.inspection_fingerprint)
        OR (failure.inspection_generation IS NULL AND failure.inspection_fingerprint IS NULL
            AND NEW.close_outcome = 'close_incomplete'))
      AND ((NEW.close_outcome = 'archived_cleanup_attention'
            AND failure.stop_certainty = 'conversation_and_processes_stopped'
            AND product.ordinary_lifecycle = 'history')
        OR (NEW.close_outcome = 'close_incomplete'
            AND failure.stop_certainty = 'shutdown_uncertain'
            AND product.ordinary_lifecycle = 'open'))
      AND NOT EXISTS (
          SELECT 1 FROM close_attempt_participants participant
          JOIN conversations conversation ON conversation.id = participant.conversation_id
          WHERE participant.attempt_id = NEW.attempt_id
            AND conversation.archived <> (NEW.close_outcome = 'archived_cleanup_attention')
      )
      AND ((failure.authority_kind = 'attempt_interrupted'
            AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
                WHERE child.failure_occurrence_id = failure.failure_occurrence_id))
        OR (failure.authority_kind <> 'attempt_interrupted'
            AND EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
                WHERE child.failure_occurrence_id = failure.failure_occurrence_id
                  AND child.scope = failure.scope
                  AND child.resource_kind = failure.resource_kind
                  AND child.identity_kind = failure.identity_kind
                  AND child.identity_codec = failure.identity_codec
                  AND child.identity_value = failure.identity_value
                  AND child.disposition IN ('failed', 'residual', 'unknown'))))
      AND EXISTS (SELECT 1 FROM messages message
          JOIN close_attempt_members member ON member.conversation_id = message.conversation_id
          WHERE member.attempt_id = NEW.attempt_id AND member.member_role IN ('latest', 'root_latest')
            AND message.message_id = 'close-outcome:' || NEW.attempt_id AND message.message_type = 'system')
))
BEGIN
    SELECT RAISE(ABORT, 'cleanup failure completion requires exact terminal disposition');
END;
