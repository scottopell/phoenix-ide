CREATE TABLE close_writer_observations (
    product_conversation_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL,
    run_ordinal INTEGER NOT NULL,
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL,
    inspection_fingerprint TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind = 'worktree'),
    identity_kind TEXT NOT NULL CHECK (identity_kind = 'worktree'),
    identity_codec TEXT NOT NULL CHECK (identity_codec = 'worktree_id_v1'),
    identity_value TEXT NOT NULL,
    detector TEXT NOT NULL CHECK (detector IN (
        'linux_descriptor', 'linux_cwd', 'linux_mapping',
        'mac_descriptor', 'mac_cwd', 'mac_mapping')),
    pid INTEGER CHECK (pid IS NULL OR (typeof(pid) = 'integer' AND pid BETWEEN 1 AND 4294967295)),
    incarnation TEXT,
    executable BLOB,
    matched_path BLOB,
    matched_kind TEXT,
    access TEXT,
    authority TEXT,
    gap TEXT,
    PRIMARY KEY (attempt_id, run_ordinal, scope, detector),
    FOREIGN KEY (attempt_id, run_ordinal) REFERENCES close_runs(attempt_id, run_ordinal) ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_value)
        REFERENCES close_retirement_resource_dispatches(attempt_id, scope, inspection_generation,
                 inspection_fingerprint, resource_kind, identity_kind, identity_value) ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
    CHECK (
        (gap IS NOT NULL AND gap IN ('unavailable', 'malformed', 'process_changed', 'descriptor_changed', 'unsupported')
         AND incarnation IS NULL AND executable IS NULL AND matched_path IS NULL
         AND matched_kind IS NULL AND access IS NULL AND authority IS NULL)
        OR
        (gap IS NULL AND pid IS NOT NULL AND incarnation IS NOT NULL AND length(incarnation) > 0
         AND executable IS NOT NULL AND typeof(executable) = 'blob' AND length(executable) > 0
         AND instr(executable, x'00') = 0
         AND matched_path IS NOT NULL AND typeof(matched_path) = 'blob' AND length(matched_path) > 0
         AND instr(matched_path, x'00') = 0
         AND matched_kind IS NOT NULL AND access IS NOT NULL AND authority IS NOT NULL
         AND (
            (detector IN ('linux_descriptor', 'mac_descriptor') AND (
                (matched_kind = 'directory' AND access IN ('read_only', 'write_only', 'read_write') AND authority = 'namespace')
                OR (matched_kind = 'regular_file' AND access IN ('write_only', 'read_write') AND authority = 'content')))
            OR (detector IN ('linux_cwd', 'mac_cwd') AND matched_kind = 'cwd' AND access = 'relative_lookup' AND authority = 'namespace')
            OR (detector IN ('linux_mapping', 'mac_mapping') AND matched_kind = 'mapping' AND access = 'write_shared' AND authority = 'content')
         ))
    )
);

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
    JOIN close_retirement_inventories inventory ON inventory.attempt_id = dispatch.attempt_id
      AND inventory.scope = dispatch.scope AND inventory.inspection_generation = dispatch.inspection_generation
      AND inventory.inspection_fingerprint = dispatch.inspection_fingerprint AND inventory.sealed = 1
    WHERE run.attempt_id = NEW.attempt_id AND run.run_ordinal = NEW.run_ordinal AND run.status = 'running'
      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)
      AND product.kind = 'ordinary' AND product.id = NEW.product_conversation_id
      AND obligation.inspection_generation = NEW.inspection_generation
      AND obligation.inspection_fingerprint = NEW.inspection_fingerprint
      AND captured.captured_worktree_identity = NEW.identity_value
      AND ((run.run_ordinal = 1 AND obligation.phase = 'retirement_requested' AND product.ordinary_lifecycle = 'open')
        OR (run.run_ordinal > 1 AND obligation.phase = 'completed'
          AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')
          AND EXISTS (SELECT 1 FROM close_run_retry_effects effect
            WHERE effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal
              AND effect.scope = NEW.scope AND effect.resource_kind = NEW.resource_kind
              AND effect.identity_kind = NEW.identity_kind AND effect.identity_codec = NEW.identity_codec
              AND effect.identity_value = NEW.identity_value
              AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
                WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
                  AND success.ordinal = effect.ordinal))))
)
BEGIN
    SELECT RAISE(ABORT, 'writer observation lacks exact active Close authority');
END;

CREATE TRIGGER close_writer_observations_no_replace
BEFORE INSERT ON close_writer_observations
WHEN EXISTS (
    SELECT 1 FROM close_writer_observations
    WHERE attempt_id = NEW.attempt_id AND run_ordinal = NEW.run_ordinal
      AND scope = NEW.scope AND detector = NEW.detector
)
BEGIN
    SELECT RAISE(ABORT, 'Close writer observations are immutable');
END;

CREATE TRIGGER close_writer_observations_no_update BEFORE UPDATE ON close_writer_observations
BEGIN
    SELECT RAISE(ABORT, 'Close writer observations are immutable');
END;
-- Only the aggregate's actual deletion cascade may remove its retained evidence.
-- Deferred run/dispatch FKs permit existing teardown ordering, not orphan commits.
CREATE TRIGGER close_writer_observations_no_delete AFTER DELETE ON close_writer_observations
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.product_conversation_id)
BEGIN
    SELECT RAISE(ABORT, 'Close writer observations are immutable until permanent Delete');
END;
