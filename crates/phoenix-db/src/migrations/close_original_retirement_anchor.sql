CREATE TABLE close_original_retirement_anchors (
    attempt_id TEXT NOT NULL PRIMARY KEY
        REFERENCES close_obligations(attempt_id) ON DELETE CASCADE,
    inspection_generation TEXT NOT NULL CHECK (inspection_generation <> ''),
    inspection_fingerprint TEXT NOT NULL CHECK (inspection_fingerprint <> '')
) WITHOUT ROWID;

CREATE TRIGGER close_original_retirement_anchors_require_complete_inventory
BEFORE INSERT ON close_original_retirement_anchors
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    WHERE obligation.attempt_id = NEW.attempt_id
      AND obligation.phase IN ('retirement_requested', 'needs_repair')
      AND obligation.inspection_generation = NEW.inspection_generation
      AND obligation.inspection_fingerprint = NEW.inspection_fingerprint
)
OR EXISTS (
    SELECT 1 FROM close_retirement_inventories prior
    WHERE prior.attempt_id = NEW.attempt_id
      AND (prior.inspection_generation <> NEW.inspection_generation
           OR prior.inspection_fingerprint <> NEW.inspection_fingerprint)
)
OR EXISTS (
    SELECT 1 FROM close_attempt_scopes target
    WHERE target.attempt_id = NEW.attempt_id
      AND NOT EXISTS (
          SELECT 1 FROM close_retirement_inventories inventory
          WHERE inventory.attempt_id = NEW.attempt_id
            AND inventory.scope = target.scope
            AND inventory.inspection_generation = NEW.inspection_generation
            AND inventory.inspection_fingerprint = NEW.inspection_fingerprint
            AND inventory.sealed = 1
      )
)
OR EXISTS (
    SELECT 1 FROM close_retirement_inventories inventory
    WHERE inventory.attempt_id = NEW.attempt_id
      AND inventory.inspection_generation = NEW.inspection_generation
      AND inventory.inspection_fingerprint = NEW.inspection_fingerprint
      AND NOT EXISTS (
          SELECT 1 FROM close_attempt_scopes target
          WHERE target.attempt_id = NEW.attempt_id
            AND target.scope = inventory.scope
      )
)
OR EXISTS (
    SELECT 1 FROM close_attempt_scopes target
    WHERE target.attempt_id = NEW.attempt_id
      AND (
          (
              SELECT COUNT(*) FROM close_expected_retirement_resources expected
              WHERE expected.attempt_id = NEW.attempt_id
                AND expected.scope = target.scope
                AND expected.inspection_generation = NEW.inspection_generation
                AND expected.inspection_fingerprint = NEW.inspection_fingerprint
                AND expected.resource_kind = 'work_scope'
          ) <> 1
          OR (
              SELECT COUNT(*) FROM close_expected_retirement_resources expected
              WHERE expected.attempt_id = NEW.attempt_id
                AND expected.scope = target.scope
                AND expected.inspection_generation = NEW.inspection_generation
                AND expected.inspection_fingerprint = NEW.inspection_fingerprint
                AND expected.resource_kind = 'work_scope'
                AND expected.identity_kind = 'opaque'
                AND expected.identity_codec = 'opaque_string_v1'
                AND expected.identity_value = target.scope
          ) <> 1
      )
)
OR EXISTS (
    SELECT 1 FROM close_expected_retirement_resources expected
    WHERE expected.attempt_id = NEW.attempt_id
      AND expected.inspection_generation = NEW.inspection_generation
      AND expected.inspection_fingerprint = NEW.inspection_fingerprint
      AND NOT EXISTS (
          SELECT 1 FROM close_attempt_scopes target
          WHERE target.attempt_id = NEW.attempt_id
            AND target.scope = expected.scope
      )
)
BEGIN
    SELECT RAISE(ABORT, 'original retirement anchor requires the exact complete sealed inventory');
END;

CREATE TRIGGER close_original_retirement_anchors_immutable
BEFORE UPDATE ON close_original_retirement_anchors
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'original retirement anchor is immutable');
END;

CREATE TRIGGER close_original_retirement_anchors_no_replacement
BEFORE INSERT ON close_original_retirement_anchors
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_original_retirement_anchors WHERE attempt_id = NEW.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'original retirement anchor cannot be replaced');
END;

CREATE TRIGGER close_original_retirement_anchors_no_direct_delete
BEFORE DELETE ON close_original_retirement_anchors
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'original retirement anchor must be retained with its obligation');
END;
CREATE TABLE close_retirement_anchor_exclusions (
    attempt_id TEXT NOT NULL PRIMARY KEY REFERENCES close_obligations(attempt_id) ON DELETE CASCADE
) WITHOUT ROWID;
INSERT INTO close_retirement_anchor_exclusions (attempt_id)
SELECT obligation.attempt_id FROM close_obligations obligation
WHERE NOT EXISTS (SELECT 1 FROM close_original_retirement_anchors anchor WHERE anchor.attempt_id = obligation.attempt_id);
CREATE TRIGGER close_retirement_anchor_exclusions_immutable BEFORE UPDATE ON close_retirement_anchor_exclusions
BEGIN SELECT RAISE(ABORT, 'retirement anchor exclusion is immutable'); END;
CREATE TRIGGER close_retirement_anchor_exclusions_no_duplicate BEFORE INSERT ON close_retirement_anchor_exclusions
WHEN EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = NEW.attempt_id)
BEGIN SELECT RAISE(ABORT, 'retirement anchor exclusion cannot be replaced'); END;
CREATE TRIGGER close_retirement_anchor_exclusions_no_direct_delete BEFORE DELETE ON close_retirement_anchor_exclusions
WHEN EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
BEGIN SELECT RAISE(ABORT, 'retirement anchor exclusion must be retained with its obligation'); END;
CREATE TRIGGER close_retirement_anchor_exclusions_no_anchor BEFORE INSERT ON close_retirement_anchor_exclusions
WHEN EXISTS (SELECT 1 FROM close_original_retirement_anchors WHERE attempt_id = NEW.attempt_id)
BEGIN SELECT RAISE(ABORT, 'original retirement anchor cannot be excluded'); END;
CREATE TRIGGER close_original_retirement_anchors_no_exclusion BEFORE INSERT ON close_original_retirement_anchors
WHEN EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions WHERE attempt_id = NEW.attempt_id)
BEGIN SELECT RAISE(ABORT, 'excluded attempt cannot acquire an original retirement anchor'); END;
