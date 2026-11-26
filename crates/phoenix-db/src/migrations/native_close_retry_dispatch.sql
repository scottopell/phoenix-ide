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
    WHERE plan.attempt_id = NEW.attempt_id
      AND receipt.intent = 'explicit_retry' AND receipt.product_conversation_id = obligation.product_conversation_id
      AND obligation.native_managed = 1 AND obligation.phase = 'completed'
      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
      AND NOT EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = plan.attempt_id)
      AND run.status = 'running' AND run.retry_evidence_kind = 'resource_plan'
      AND NEW.dispatched_at_us >= run.created_at_us
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
      AND effect.resource_kind = 'worktree' AND effect.identity_kind = 'worktree'
      AND effect.identity_codec = 'worktree_id_v1' AND effect.identity_value = captured.captured_worktree_identity
      AND captured.captured_worktree_fingerprint IS NOT NULL AND captured.captured_worktree_locator IS NOT NULL
      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
          WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
            AND success.ordinal = effect.ordinal)
