-- A capture-origin plan is not an original anchor, inventory, or retirement proof.
-- The stopped inspection and failure are never updated by this protocol.
CREATE TABLE native_close_captured_loss_retry_plans (
    attempt_id TEXT NOT NULL,
    run_ordinal INTEGER NOT NULL CHECK (run_ordinal = 2),
    origin_generation TEXT NOT NULL CHECK (length(origin_generation) > 0),
    origin_fingerprint TEXT NOT NULL CHECK (length(origin_fingerprint) > 0),
    inspection_generation TEXT NOT NULL CHECK (length(inspection_generation) > 0),
    inspection_fingerprint TEXT NOT NULL CHECK (length(inspection_fingerprint) > 0),
    observed_at_us INTEGER NOT NULL CHECK (observed_at_us >= 0),
    PRIMARY KEY (attempt_id, run_ordinal),
    FOREIGN KEY (attempt_id, run_ordinal) REFERENCES native_close_retry_plans(attempt_id, run_ordinal)
        ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE native_close_captured_loss_retry_inspections (
    attempt_id TEXT NOT NULL,
    run_ordinal INTEGER NOT NULL,
    scope TEXT NOT NULL,
    generation TEXT NOT NULL CHECK (length(generation) > 0),
    fingerprint TEXT NOT NULL CHECK (length(fingerprint) > 0),
    PRIMARY KEY (attempt_id, run_ordinal, scope),
    FOREIGN KEY (attempt_id, run_ordinal) REFERENCES native_close_captured_loss_retry_plans(attempt_id, run_ordinal) ON DELETE CASCADE,
    FOREIGN KEY (attempt_id, scope) REFERENCES close_attempt_scopes(attempt_id, scope) ON DELETE CASCADE
);
CREATE TABLE native_close_captured_loss_retry_resources (
    attempt_id TEXT NOT NULL,
    run_ordinal INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    scope TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('worktree', 'work_scope')),
    identity_kind TEXT NOT NULL,
    identity_codec TEXT NOT NULL,
    identity_value TEXT NOT NULL,
    PRIMARY KEY (attempt_id, run_ordinal, ordinal),
    UNIQUE (attempt_id, run_ordinal, scope, resource_kind),
    FOREIGN KEY (attempt_id, run_ordinal) REFERENCES native_close_captured_loss_retry_plans(attempt_id, run_ordinal) ON DELETE CASCADE,
    FOREIGN KEY (attempt_id, scope) REFERENCES close_attempt_scopes(attempt_id, scope) ON DELETE CASCADE
);

CREATE TRIGGER native_close_captured_loss_retry_origin_requires_exact_stop
BEFORE INSERT ON native_close_captured_loss_retry_plans
WHEN EXISTS (SELECT 1 FROM native_close_captured_loss_retry_plans WHERE attempt_id = NEW.attempt_id)
  OR EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id)
  OR EXISTS (SELECT 1 FROM native_close_completion_retry_admissions WHERE attempt_id = NEW.attempt_id)
  OR NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN close_runs run ON run.attempt_id = obligation.attempt_id AND run.run_ordinal = 1
    JOIN native_close_initial_receipts receipt ON receipt.attempt_id = run.attempt_id
        AND receipt.run_ordinal = run.run_ordinal AND receipt.intent = 'initial'
        AND receipt.product_conversation_id = obligation.product_conversation_id
    JOIN native_close_completion_retry_origins initial ON initial.attempt_id = receipt.attempt_id
        AND initial.origin_request_id = receipt.request_id
        AND initial.origin_request_fingerprint = receipt.request_fingerprint
        AND initial.product_conversation_id = receipt.product_conversation_id
        AND initial.source_transcript_id = receipt.source_transcript_id
        AND initial.source_message_id = receipt.source_message_id
        AND initial.source_tool_use_id = receipt.source_tool_use_id
        AND initial.initial_run_ordinal = receipt.run_ordinal
    JOIN close_completion_retry_allowances allowance ON allowance.attempt_id = run.attempt_id
        AND allowance.suppressed = 0
    JOIN close_cleanup_failures failure ON failure.attempt_id = run.attempt_id
        AND failure.cleanup_run_ordinal = run.run_ordinal
        AND failure.source_product_conversation_id = obligation.product_conversation_id
        AND failure.failure_occurrence_id = 'close-failure:1:' || run.attempt_id
    JOIN close_attempt_scopes failed_scope ON failed_scope.attempt_id = run.attempt_id
        AND failed_scope.scope = failure.scope
    JOIN close_retirement_losses loss ON loss.attempt_id = failed_scope.attempt_id
        AND loss.scope = failed_scope.scope
    JOIN close_retirement_inspections inspection ON inspection.attempt_id = loss.attempt_id
        AND inspection.scope = loss.scope AND inspection.generation = loss.generation
    WHERE obligation.attempt_id = NEW.attempt_id AND obligation.native_managed = 1
        AND obligation.topology_sealed = 1 AND obligation.phase = 'completed'
        AND obligation.close_outcome = 'close_incomplete'
        AND obligation.inspection_generation = NEW.origin_generation
        AND obligation.inspection_fingerprint = NEW.origin_fingerprint
        AND run.status = 'stopped' AND run.ended_at_us = failure.occurred_at_us
        AND failure.authority_kind = 'captured_scope'
        AND failure.reason = 'manual_repair_required'
        AND failure.resource_kind = 'work_scope' AND failure.identity_kind = 'opaque'
        AND failure.identity_codec = 'opaque_string_v1' AND failure.identity_value = failure.scope
        AND failure.inspection_generation IS NULL AND failure.inspection_fingerprint IS NULL
        AND length(trim(failure.detail)) > 0
        AND failure.stop_certainty = 'shutdown_uncertain' AND failure.confirmed_at_us IS NULL
        AND NEW.observed_at_us > failure.occurred_at_us
        AND NOT EXISTS (SELECT 1 FROM close_runs WHERE attempt_id = NEW.attempt_id AND run_ordinal > 1)
        AND NOT EXISTS (SELECT 1 FROM close_original_retirement_anchors WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_retirement_inventories WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_expected_retirement_resources WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_retirement_resource_dispatches WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_retirement_resources WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_retirement_resource_history WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_process_step_successes WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_run_retry_effects WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes WHERE attempt_id = NEW.attempt_id)
        AND (SELECT COUNT(*) FROM close_cleanup_failure_resources child
            WHERE child.failure_occurrence_id = failure.failure_occurrence_id)
            = (SELECT SUM(1 + (captured_worktree_identity IS NOT NULL))
                FROM close_attempt_scopes WHERE attempt_id = NEW.attempt_id)
        AND NOT EXISTS (SELECT 1 FROM close_cleanup_failure_resources child
            WHERE child.failure_occurrence_id = failure.failure_occurrence_id
              AND (child.disposition <> 'unknown' OR NOT EXISTS (
                SELECT 1 FROM close_attempt_scopes captured
                WHERE captured.attempt_id = NEW.attempt_id AND captured.scope = child.scope
                  AND ((child.resource_kind = 'work_scope' AND child.identity_kind = 'opaque'
                    AND child.identity_codec = 'opaque_string_v1' AND child.identity_value = captured.scope)
                   OR (child.resource_kind = 'worktree' AND child.identity_kind = 'worktree'
                    AND child.identity_codec = 'worktree_id_v1'
                    AND child.identity_value = captured.captured_worktree_identity
                    AND captured.captured_worktree_fingerprint IS NOT NULL
                    AND captured.captured_worktree_locator IS NOT NULL))
                  AND child.ordinal = (SELECT COALESCE(SUM(1 + (prior.captured_worktree_identity IS NOT NULL)), 0)
                    FROM close_attempt_scopes prior WHERE prior.attempt_id = NEW.attempt_id
                      AND prior.scope < child.scope) + (child.resource_kind = 'worktree'))))
        AND NOT EXISTS (SELECT 1 FROM close_attempt_members member JOIN conversations conversation
            ON conversation.id = member.conversation_id WHERE member.attempt_id = NEW.attempt_id
            AND (conversation.archived <> 1 OR conversation.state_kind NOT IN ('Idle', 'Closed', 'Error')))
)
BEGIN
    SELECT RAISE(ABORT, 'captured-loss retry requires exact effect-free captured stop');
END;

CREATE TRIGGER native_close_captured_loss_retry_inspection_requires_fresh_scope
BEFORE INSERT ON native_close_captured_loss_retry_inspections
WHEN EXISTS (SELECT 1 FROM native_close_captured_loss_retry_inspections
    WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal AND scope = NEW.scope)
  OR EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
 OR NOT EXISTS (SELECT 1 FROM close_attempt_scopes captured
    JOIN close_retirement_inspections original ON original.attempt_id = captured.attempt_id AND original.scope = captured.scope
    WHERE captured.attempt_id = NEW.attempt_id AND captured.scope = NEW.scope
      AND captured.captured_worktree_identity IS NOT NULL
      AND captured.captured_worktree_fingerprint IS NOT NULL AND captured.captured_worktree_locator IS NOT NULL
      AND NEW.generation <> original.generation)
BEGIN
    SELECT RAISE(ABORT, 'captured-loss retry requires fresh full captured scope inspection');
END;

CREATE TRIGGER native_close_captured_loss_retry_resource_requires_finite_identity
BEFORE INSERT ON native_close_captured_loss_retry_resources
WHEN EXISTS (SELECT 1 FROM native_close_captured_loss_retry_resources
    WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal
      AND (ordinal = NEW.ordinal OR (scope = NEW.scope AND resource_kind = NEW.resource_kind)))
  OR EXISTS (SELECT 1 FROM native_close_retry_plans WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal)
 OR NOT EXISTS (SELECT 1 FROM close_attempt_scopes captured
    WHERE captured.attempt_id = NEW.attempt_id AND captured.scope = NEW.scope
      AND ((NEW.resource_kind = 'worktree' AND NEW.identity_kind = 'worktree'
        AND NEW.identity_codec = 'worktree_id_v1' AND NEW.identity_value = captured.captured_worktree_identity
        AND captured.captured_worktree_fingerprint IS NOT NULL AND captured.captured_worktree_locator IS NOT NULL)
       OR (NEW.resource_kind = 'work_scope' AND NEW.identity_kind = 'opaque'
        AND NEW.identity_codec = 'opaque_string_v1' AND NEW.identity_value = captured.scope))
      AND NEW.ordinal = (SELECT COALESCE(SUM(1 + (prior.captured_worktree_identity IS NOT NULL)), 0)
        FROM close_attempt_scopes prior WHERE prior.attempt_id = NEW.attempt_id AND prior.scope < NEW.scope)
        + (NEW.resource_kind = 'work_scope' AND captured.captured_worktree_identity IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'captured-loss retry resources require finite ordered captured identities');
END;

-- These projections are used ONLY by the native explicit retry guards. They do
-- not write or expose an original retirement anchor or StoppedClose proof.
CREATE VIEW native_close_retry_snapshot_authorities AS
SELECT plan.attempt_id, plan.run_ordinal, anchor.inspection_generation, anchor.inspection_fingerprint
FROM native_close_retry_plans plan JOIN close_original_retirement_anchors anchor
 ON anchor.attempt_id = plan.attempt_id AND anchor.inspection_generation = plan.inspection_generation
 AND anchor.inspection_fingerprint = plan.inspection_fingerprint
WHERE NOT EXISTS (SELECT 1 FROM native_close_captured_loss_retry_plans capture
    WHERE capture.attempt_id = plan.attempt_id AND capture.run_ordinal = plan.run_ordinal)
UNION ALL
SELECT capture.attempt_id, capture.run_ordinal, capture.inspection_generation, capture.inspection_fingerprint
FROM native_close_captured_loss_retry_plans capture
JOIN native_close_retry_plans plan USING (attempt_id, run_ordinal, inspection_generation, inspection_fingerprint)
WHERE plan.failed_run_ordinal = 1 AND plan.run_ordinal = 2 AND plan.evidence_kind = 'resource_plan'
  AND plan.effect_count = (SELECT COUNT(*) FROM native_close_captured_loss_retry_resources resource
      WHERE resource.attempt_id = capture.attempt_id AND resource.run_ordinal = capture.run_ordinal)
  AND NOT EXISTS (SELECT 1 FROM close_original_retirement_anchors WHERE attempt_id = capture.attempt_id);

CREATE VIEW native_close_retry_resource_authorities AS
SELECT expected.* FROM close_expected_retirement_resources expected
UNION ALL
SELECT resource.attempt_id, resource.scope, capture.inspection_generation, capture.inspection_fingerprint,
    resource.resource_kind, resource.identity_kind, resource.identity_codec, resource.identity_value
FROM native_close_captured_loss_retry_resources resource
JOIN native_close_retry_snapshot_authorities capture USING (attempt_id, run_ordinal)
JOIN close_attempt_scopes captured ON captured.attempt_id = resource.attempt_id AND captured.scope = resource.scope
WHERE EXISTS (SELECT 1 FROM native_close_captured_loss_retry_plans origin
    WHERE origin.attempt_id = resource.attempt_id AND origin.run_ordinal = resource.run_ordinal)
  AND ((resource.resource_kind = 'work_scope' AND resource.identity_kind = 'opaque'
      AND resource.identity_codec = 'opaque_string_v1' AND resource.identity_value = captured.scope)
    OR (resource.resource_kind = 'worktree' AND resource.identity_kind = 'worktree'
      AND resource.identity_codec = 'worktree_id_v1' AND resource.identity_value = captured.captured_worktree_identity
      AND captured.captured_worktree_fingerprint IS NOT NULL AND captured.captured_worktree_locator IS NOT NULL));

CREATE VIEW native_close_retry_scope_authorities AS
SELECT attempt_id, scope, inspection_generation, inspection_fingerprint, sealed
FROM close_retirement_inventories
UNION ALL
SELECT capture.attempt_id, scope.scope, capture.inspection_generation, capture.inspection_fingerprint, 1
FROM native_close_retry_snapshot_authorities capture
JOIN native_close_captured_loss_retry_plans origin USING (attempt_id, run_ordinal)
JOIN close_attempt_scopes scope ON scope.attempt_id = capture.attempt_id;

-- Replaces the dispatch composite FK for EVERY dispatch, not just native retries.
-- The independent phase/source/receipt/latest-run trigger remains mandatory.
CREATE TRIGGER close_retirement_resource_dispatches_require_full_membership
BEFORE INSERT ON close_retirement_resource_dispatches
WHEN NOT EXISTS (
    SELECT 1 FROM native_close_retry_resource_authorities expected
    WHERE expected.attempt_id = NEW.attempt_id AND expected.scope = NEW.scope
      AND expected.inspection_generation = NEW.inspection_generation
      AND expected.inspection_fingerprint = NEW.inspection_fingerprint
      AND expected.resource_kind = NEW.resource_kind AND expected.identity_kind = NEW.identity_kind
      AND expected.identity_codec = NEW.identity_codec AND expected.identity_value = NEW.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'retirement dispatch requires exact full resource and snapshot membership');
END;

CREATE TRIGGER close_retirement_resource_dispatches_updates_require_full_membership
BEFORE UPDATE ON close_retirement_resource_dispatches
WHEN NOT EXISTS (
    SELECT 1 FROM native_close_retry_resource_authorities expected
    WHERE expected.attempt_id = NEW.attempt_id AND expected.scope = NEW.scope
      AND expected.inspection_generation = NEW.inspection_generation
      AND expected.inspection_fingerprint = NEW.inspection_fingerprint
      AND expected.resource_kind = NEW.resource_kind AND expected.identity_kind = NEW.identity_kind
      AND expected.identity_codec = NEW.identity_codec AND expected.identity_value = NEW.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'retirement dispatch requires exact full resource and snapshot membership');
END;

-- The existing immutable guard rejects every expected-resource UPDATE. These
-- reference guards also preserve the removed FK during aggregate cleanup,
-- when the product-scoped standalone-delete guard no longer applies.
CREATE TRIGGER close_expected_retirement_resources_preserve_dispatched_update
BEFORE UPDATE ON close_expected_retirement_resources
WHEN EXISTS (
    SELECT 1 FROM close_retirement_resource_dispatches dispatch
    WHERE dispatch.attempt_id = OLD.attempt_id AND dispatch.scope = OLD.scope
      AND dispatch.inspection_generation = OLD.inspection_generation
      AND dispatch.inspection_fingerprint = OLD.inspection_fingerprint
      AND dispatch.resource_kind = OLD.resource_kind AND dispatch.identity_kind = OLD.identity_kind
      AND dispatch.identity_value = OLD.identity_value
)
AND (NEW.attempt_id <> OLD.attempt_id OR NEW.scope <> OLD.scope
  OR NEW.inspection_generation <> OLD.inspection_generation
  OR NEW.inspection_fingerprint <> OLD.inspection_fingerprint
  OR NEW.resource_kind <> OLD.resource_kind OR NEW.identity_kind <> OLD.identity_kind
  OR NEW.identity_codec <> OLD.identity_codec OR NEW.identity_value <> OLD.identity_value)
BEGIN
    SELECT RAISE(ABORT, 'dispatched expected retirement resource must be retained unchanged');
END;

CREATE TRIGGER close_expected_retirement_resources_preserve_dispatched_delete
BEFORE DELETE ON close_expected_retirement_resources
WHEN EXISTS (
    SELECT 1 FROM close_retirement_resource_dispatches dispatch
    WHERE dispatch.attempt_id = OLD.attempt_id AND dispatch.scope = OLD.scope
      AND dispatch.inspection_generation = OLD.inspection_generation
      AND dispatch.inspection_fingerprint = OLD.inspection_fingerprint
      AND dispatch.resource_kind = OLD.resource_kind AND dispatch.identity_kind = OLD.identity_kind
      AND dispatch.identity_value = OLD.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'dispatched expected retirement resource must be retained unchanged');
END;
