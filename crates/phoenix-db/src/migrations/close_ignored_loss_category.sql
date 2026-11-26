DROP TRIGGER close_obligations_invalidate_inspection_on_reentry;
DROP TRIGGER close_retirement_losses_require_open_inspection_on_insert;
DROP TRIGGER close_retirement_losses_require_open_inspection_on_update;
DROP TRIGGER close_retirement_losses_require_open_inspection_on_delete;
DROP TRIGGER close_retirement_losses_reject_nul_git_path;
DROP TRIGGER close_retirement_losses_reject_nul_git_path_on_update;

CREATE TABLE close_retirement_losses_backup AS
SELECT attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
FROM close_retirement_losses;

DROP TABLE close_retirement_losses;

CREATE TABLE close_retirement_losses (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    generation TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN (
        'staged_tracked_paths',
        'unstaged_tracked_paths',
        'untracked_non_ignored_paths',
        'ignored',
        'initialized_submodule_state',
        'detached_unreachable_commits'
    )),
    identity_kind TEXT NOT NULL CHECK (identity_kind IN ('git_path', 'git_oid', 'opaque', 'worktree')),
    identity_codec TEXT NOT NULL CHECK (
        (identity_kind = 'git_path' AND identity_codec = 'git_path_bytes_hex_v1')
        OR (identity_kind = 'git_oid' AND identity_codec = 'hex')
        OR (identity_kind = 'opaque' AND identity_codec = 'opaque_string_v1')
        OR (identity_kind = 'worktree' AND identity_codec = 'worktree_id_v1')
    ),
    identity_value TEXT NOT NULL CHECK (
        identity_value <> ''
        AND (
            (
                identity_kind = 'git_path'
                AND SUBSTR(identity_value, 1, LENGTH('git_path_bytes_hex_v1:')) = 'git_path_bytes_hex_v1:'
                AND LENGTH(SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1)) > 0
                AND LENGTH(SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1)) % 2 = 0
                AND SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1) NOT GLOB '*[^0-9a-f]*'
            )
            OR (identity_kind = 'git_oid' AND LOWER(identity_value) GLOB REPLACE(HEX(ZEROBLOB(LENGTH(identity_value)/2)), '0', '[0-9a-f]'))
            OR (identity_kind = 'opaque' AND identity_value <> '')
            OR (identity_kind = 'worktree' AND identity_value <> '')
        )
    ),
    PRIMARY KEY (attempt_id, scope, generation, category, identity_kind, identity_value),
    FOREIGN KEY (attempt_id, scope, generation)
        REFERENCES close_retirement_inspections(attempt_id, scope, generation)
        ON DELETE CASCADE,
    CHECK (
        identity_kind <> 'git_oid'
        OR (LENGTH(identity_value) IN (40, 64) AND LOWER(identity_value) = identity_value)
    ),
    CHECK (
        (
            category IN (
                'staged_tracked_paths',
                'unstaged_tracked_paths',
                'untracked_non_ignored_paths',
                'ignored',
                'initialized_submodule_state'
            )
            AND identity_kind = 'git_path'
        )
        OR (
            category = 'detached_unreachable_commits'
            AND identity_kind = 'git_oid'
        )
    )
);

INSERT INTO close_retirement_losses (
    attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
)
SELECT attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
FROM close_retirement_losses_backup;

DROP TABLE close_retirement_losses_backup;

CREATE TRIGGER close_retirement_losses_require_open_inspection_on_insert
BEFORE INSERT ON close_retirement_losses
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = NEW.attempt_id AND phase = 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'close loss inventory is sealed outside inspection replacement');
END;

CREATE TRIGGER close_retirement_losses_require_open_inspection_on_update
BEFORE UPDATE ON close_retirement_losses
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = OLD.attempt_id AND phase = 'awaiting_retirement_inspection'
) OR NOT EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = NEW.attempt_id AND phase = 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'close loss inventory is sealed outside inspection replacement');
END;

CREATE TRIGGER close_retirement_losses_require_open_inspection_on_delete
BEFORE DELETE ON close_retirement_losses
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'close loss inventory is sealed outside inspection replacement');
END;

CREATE TRIGGER close_retirement_losses_reject_nul_git_path
BEFORE INSERT ON close_retirement_losses
FOR EACH ROW
WHEN NEW.identity_kind = 'git_path'
  AND EXISTS (
      WITH RECURSIVE byte_pos(pos) AS (
          SELECT LENGTH('git_path_bytes_hex_v1:') + 1
          UNION ALL
          SELECT pos + 2 FROM byte_pos
          WHERE pos + 2 <= LENGTH(NEW.identity_value)
      )
      SELECT 1 FROM byte_pos
      WHERE SUBSTR(NEW.identity_value, pos, 2) = '00'
  )
BEGIN
    SELECT RAISE(ABORT, 'Git path identity cannot contain a NUL byte');
END;

CREATE TRIGGER close_retirement_losses_reject_nul_git_path_on_update
BEFORE UPDATE OF identity_kind, identity_value ON close_retirement_losses
FOR EACH ROW
WHEN NEW.identity_kind = 'git_path'
  AND EXISTS (
      WITH RECURSIVE byte_pos(pos) AS (
          SELECT LENGTH('git_path_bytes_hex_v1:') + 1
          UNION ALL
          SELECT pos + 2 FROM byte_pos
          WHERE pos + 2 <= LENGTH(NEW.identity_value)
      )
      SELECT 1 FROM byte_pos
      WHERE SUBSTR(NEW.identity_value, pos, 2) = '00'
  )
BEGIN
    SELECT RAISE(ABORT, 'Git path identity cannot contain a NUL byte');
END;

CREATE TRIGGER close_obligations_invalidate_inspection_on_reentry
AFTER UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'awaiting_retirement_inspection'
  AND OLD.phase <> 'awaiting_retirement_inspection'
  AND OLD.phase <> 'needs_repair'
BEGIN
    DELETE FROM close_retirement_inspections WHERE attempt_id = NEW.attempt_id;
    UPDATE close_obligations
    SET inspection_generation = NULL, inspection_fingerprint = NULL
    WHERE attempt_id = NEW.attempt_id;
END;
