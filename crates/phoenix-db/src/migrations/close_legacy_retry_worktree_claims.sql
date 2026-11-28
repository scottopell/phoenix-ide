CREATE TABLE close_legacy_retry_worktree_claims (
    attempt_id TEXT NOT NULL,
    run_ordinal INTEGER NOT NULL DEFAULT 1 CHECK (run_ordinal = 1),
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL,
    inspection_fingerprint TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind = 'worktree'),
    identity_kind TEXT NOT NULL CHECK (identity_kind = 'worktree'),
    identity_codec TEXT NOT NULL,
    identity_value TEXT NOT NULL,
    continuation_kind TEXT NOT NULL CHECK (continuation_kind = 'legacy_initial'),
    PRIMARY KEY (attempt_id, run_ordinal, scope, resource_kind, identity_kind, identity_codec, identity_value),
    FOREIGN KEY (attempt_id, run_ordinal) REFERENCES close_runs (attempt_id, run_ordinal) ON DELETE RESTRICT,
    FOREIGN KEY (attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_value)
      REFERENCES close_worktree_cleanup_plans
                 (attempt_id, scope, inspection_generation, inspection_fingerprint,
                  resource_kind, identity_kind, identity_value) ON DELETE RESTRICT
);

CREATE TRIGGER close_legacy_retry_worktree_claims_exact_cleanup_plan
BEFORE INSERT ON close_legacy_retry_worktree_claims
WHEN NOT EXISTS (
    SELECT 1 FROM close_worktree_cleanup_plans plan
    JOIN close_retirement_resource_dispatches dispatch
      ON dispatch.attempt_id = plan.attempt_id AND dispatch.scope = plan.scope
     AND dispatch.inspection_generation = plan.inspection_generation
     AND dispatch.inspection_fingerprint = plan.inspection_fingerprint
     AND dispatch.resource_kind = plan.resource_kind
     AND dispatch.identity_kind = plan.identity_kind
     AND dispatch.identity_codec = plan.identity_codec
     AND dispatch.identity_value = plan.identity_value
    WHERE plan.attempt_id = NEW.attempt_id AND plan.scope = NEW.scope
      AND plan.inspection_generation = NEW.inspection_generation
      AND plan.inspection_fingerprint = NEW.inspection_fingerprint
      AND plan.resource_kind = NEW.resource_kind AND plan.identity_kind = NEW.identity_kind
      AND plan.identity_codec = NEW.identity_codec AND plan.identity_value = NEW.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'legacy Retry claim requires exact worktree cleanup plan');
END;

CREATE TRIGGER close_legacy_retry_worktree_claims_immutable
BEFORE UPDATE ON close_legacy_retry_worktree_claims
BEGIN
    SELECT RAISE(ABORT, 'legacy Retry worktree claim is immutable');
END;

CREATE TRIGGER close_legacy_retry_worktree_claims_no_delete
BEFORE DELETE ON close_legacy_retry_worktree_claims
BEGIN
    SELECT RAISE(ABORT, 'legacy Retry worktree claim is immutable');
END;
