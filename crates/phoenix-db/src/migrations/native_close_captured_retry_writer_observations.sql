-- A captured-loss Retry has a fresh, run-scoped plan, not an original sealed inventory.
-- Continue to require the exact dispatched Worktree and a still-pending Retry effect.
DROP TRIGGER close_writer_observations_require_active_authority;

CREATE TRIGGER close_writer_observations_require_active_authority
BEFORE INSERT ON close_writer_observations
WHEN NOT EXISTS (
    SELECT 1 FROM close_runs run
    JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id
    JOIN product_conversations product ON product.id = obligation.product_conversation_id
    JOIN close_attempt_scopes captured ON captured.attempt_id = run.attempt_id AND captured.scope = NEW.scope
    JOIN close_retirement_resource_dispatches dispatch ON dispatch.attempt_id = run.attempt_id
      AND dispatch.scope = NEW.scope AND dispatch.inspection_generation = NEW.inspection_generation
      AND dispatch.inspection_fingerprint = NEW.inspection_fingerprint
      AND dispatch.resource_kind = NEW.resource_kind AND dispatch.identity_kind = NEW.identity_kind
      AND dispatch.identity_codec = NEW.identity_codec AND dispatch.identity_value = NEW.identity_value
    LEFT JOIN close_retirement_inventories inventory ON inventory.attempt_id = dispatch.attempt_id
      AND inventory.scope = dispatch.scope AND inventory.inspection_generation = dispatch.inspection_generation
      AND inventory.inspection_fingerprint = dispatch.inspection_fingerprint AND inventory.sealed = 1
      AND obligation.inspection_generation = dispatch.inspection_generation
      AND obligation.inspection_fingerprint = dispatch.inspection_fingerprint
    WHERE run.attempt_id = NEW.attempt_id AND run.run_ordinal = NEW.run_ordinal AND run.status = 'running'
      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
      AND product.kind = 'ordinary' AND product.id = NEW.product_conversation_id
      AND captured.captured_worktree_identity = NEW.identity_value
      AND ((run.run_ordinal = 1 AND obligation.phase = 'retirement_requested'
          AND product.ordinary_lifecycle = 'open' AND inventory.attempt_id IS NOT NULL)
        OR (run.run_ordinal > 1 AND obligation.phase = 'completed'
          AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
          AND EXISTS (SELECT 1 FROM close_run_retry_effects effect
            WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal
              AND effect.scope = NEW.scope AND effect.resource_kind = NEW.resource_kind
              AND effect.identity_kind = NEW.identity_kind AND effect.identity_codec = NEW.identity_codec
              AND effect.identity_value = NEW.identity_value
              AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
                WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
                  AND success.ordinal = effect.ordinal)
              AND (inventory.attempt_id IS NOT NULL OR (obligation.native_managed = 1 AND EXISTS (
                SELECT 1 FROM native_close_retry_snapshot_authorities plan
                JOIN native_close_captured_loss_retry_resources resource USING (attempt_id, run_ordinal)
                WHERE plan.attempt_id = effect.attempt_id AND plan.run_ordinal = effect.run_ordinal
                  AND plan.inspection_generation = NEW.inspection_generation
                  AND plan.inspection_fingerprint = NEW.inspection_fingerprint
                  AND resource.ordinal = effect.ordinal AND resource.scope = effect.scope
                  AND resource.resource_kind = effect.resource_kind AND resource.identity_kind = effect.identity_kind
                  AND resource.identity_codec = effect.identity_codec AND resource.identity_value = effect.identity_value
              ))))))
)
BEGIN
    SELECT RAISE(ABORT, 'writer observation lacks exact active Close authority');
END;
