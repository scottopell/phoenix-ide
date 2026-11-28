CREATE TABLE close_retry_tmux_dispatch_confirmations (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL,
    inspection_fingerprint TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind = 'tmux_server'),
    identity_kind TEXT NOT NULL CHECK (identity_kind = 'opaque'),
    identity_codec TEXT NOT NULL CHECK (identity_codec = 'opaque_string_v1'),
    identity_value TEXT NOT NULL,
    run_ordinal INTEGER NOT NULL CHECK (typeof(run_ordinal) = 'integer' AND run_ordinal > 1),
    PRIMARY KEY (attempt_id, scope, inspection_generation, inspection_fingerprint, identity_value),
    FOREIGN KEY (attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value)
      REFERENCES close_retirement_resource_dispatches
                 (attempt_id, scope, inspection_generation, inspection_fingerprint,
                  resource_kind, identity_kind, identity_codec, identity_value) ON DELETE CASCADE,
    FOREIGN KEY (attempt_id, run_ordinal) REFERENCES close_runs (attempt_id, run_ordinal)
);

CREATE TRIGGER close_retry_tmux_dispatch_confirmations_immutable
BEFORE UPDATE ON close_retry_tmux_dispatch_confirmations
BEGIN
    SELECT RAISE(ABORT, 'tmux dispatch confirmation is immutable');
END;

CREATE TRIGGER close_retry_tmux_dispatch_confirmations_require_native_effect
BEFORE INSERT ON close_retry_tmux_dispatch_confirmations
WHEN NOT EXISTS (
    SELECT 1 FROM native_close_retry_plans plan
    JOIN close_run_retry_effects effect USING (attempt_id, run_ordinal)
    WHERE plan.attempt_id = NEW.attempt_id AND plan.run_ordinal = NEW.run_ordinal
      AND plan.evidence_kind = 'resource_plan'
      AND plan.inspection_generation = NEW.inspection_generation
      AND plan.inspection_fingerprint = NEW.inspection_fingerprint
      AND effect.scope = NEW.scope AND effect.resource_kind = NEW.resource_kind
      AND effect.identity_kind = NEW.identity_kind AND effect.identity_codec = NEW.identity_codec
      AND effect.identity_value = NEW.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'tmux dispatch confirmation requires the exact native Retry effect');
END;
