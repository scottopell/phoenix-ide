CREATE TABLE coordinator_work_scope (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    work_scope_id TEXT NOT NULL UNIQUE REFERENCES work_scopes(id) ON DELETE RESTRICT
);

CREATE TRIGGER coordinator_work_scope_validate_insert
BEFORE INSERT ON coordinator_work_scope
WHEN NOT EXISTS (
    SELECT 1 FROM work_scopes scope WHERE scope.id = NEW.work_scope_id
      AND scope.environment_kind = 'coordinator_home' AND scope.authority_kind = 'work'
      AND scope.lifecycle = 'active'
) OR EXISTS (SELECT 1 FROM product_conversation_work_scopes WHERE work_scope_id = NEW.work_scope_id)
BEGIN
    SELECT RAISE(ABORT, 'Coordinator ownership requires its active home WorkScope');
END;
CREATE TRIGGER coordinator_work_scope_immutable_update BEFORE UPDATE ON coordinator_work_scope
BEGIN
    SELECT RAISE(ABORT, 'Coordinator WorkScope ownership is immutable');
END;
CREATE TRIGGER coordinator_work_scope_immutable_delete BEFORE DELETE ON coordinator_work_scope
BEGIN
    SELECT RAISE(ABORT, 'Coordinator WorkScope ownership cannot be deleted');
END;
CREATE TRIGGER coordinator_home_bind AFTER INSERT ON work_scopes
WHEN NEW.environment_kind = 'coordinator_home'
BEGIN
    INSERT INTO coordinator_work_scope (singleton, work_scope_id) VALUES (1, NEW.id);
END;
CREATE TRIGGER coordinator_home_reconcile AFTER INSERT ON coordinator_work_scope
BEGIN
    UPDATE conversations SET work_scope_id = NEW.work_scope_id WHERE runtime_role = 'coordinator';
END;
CREATE TRIGGER coordinator_home_immutable_update BEFORE UPDATE ON work_scopes
WHEN (OLD.environment_kind = 'coordinator_home' OR NEW.environment_kind = 'coordinator_home')
  AND (OLD.id IS NOT NEW.id OR OLD.environment_kind IS NOT NEW.environment_kind
    OR OLD.cwd IS NOT NEW.cwd OR OLD.authority_kind IS NOT NEW.authority_kind
    OR OLD.lifecycle IS NOT NEW.lifecycle OR OLD.retired_at IS NOT NEW.retired_at
    OR OLD.retired_reason IS NOT NEW.retired_reason OR OLD.worktree_path IS NOT NEW.worktree_path
    OR OLD.branch_name IS NOT NEW.branch_name OR OLD.base_branch IS NOT NEW.base_branch
    OR OLD.worktree_id IS NOT NEW.worktree_id OR OLD.worktree_fingerprint IS NOT NEW.worktree_fingerprint)
BEGIN
    SELECT RAISE(ABORT, 'Coordinator home WorkScope cannot be rebound, changed, or retired');
END;
CREATE TRIGGER coordinator_home_immutable_delete BEFORE DELETE ON work_scopes
WHEN OLD.environment_kind = 'coordinator_home'
BEGIN
    SELECT RAISE(ABORT, 'Coordinator home WorkScope cannot be deleted');
END;
CREATE TRIGGER coordinator_home_shape_insert BEFORE INSERT ON work_scopes
WHEN NEW.environment_kind = 'coordinator_home'
  AND (NEW.authority_kind <> 'work' OR NEW.lifecycle <> 'active'
    OR NEW.retired_at IS NOT NULL OR NEW.retired_reason IS NOT NULL
    OR NEW.cwd IS NULL OR trim(NEW.cwd) = ''
    OR NEW.worktree_path IS NOT NULL OR NEW.branch_name IS NOT NULL OR NEW.base_branch IS NOT NULL
    OR NEW.worktree_id IS NOT NULL OR NEW.worktree_fingerprint IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid Coordinator home WorkScope shape');
END;

DROP TRIGGER conversations_role_scope_insert;
DROP TRIGGER conversations_role_scope_update;
CREATE TRIGGER conversations_role_scope_insert BEFORE INSERT ON conversations
WHEN NEW.runtime_role NOT IN ('user', 'sub_agent', 'coordinator')
  OR (NEW.runtime_role = 'user' AND NEW.work_scope_id IS NULL)
  OR (NEW.runtime_role = 'coordinator' AND NEW.work_scope_id IS NOT
      (SELECT work_scope_id FROM coordinator_work_scope WHERE singleton = 1))
  OR (NEW.runtime_role <> 'coordinator' AND NEW.work_scope_id IN (SELECT work_scope_id FROM coordinator_work_scope))
  OR (NEW.coordinator_head = 1 AND NEW.runtime_role <> 'coordinator')
  OR (NEW.coordinator_head = 1 AND NEW.continued_in_conv_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid conversation runtime role/work scope');
END;
CREATE TRIGGER conversations_role_scope_update
BEFORE UPDATE OF runtime_role, work_scope_id, coordinator_head, continued_in_conv_id ON conversations
WHEN NEW.runtime_role NOT IN ('user', 'sub_agent', 'coordinator')
  OR (NEW.runtime_role = 'user' AND NEW.work_scope_id IS NULL)
  OR (NEW.runtime_role = 'coordinator' AND NEW.work_scope_id IS NOT
      (SELECT work_scope_id FROM coordinator_work_scope WHERE singleton = 1))
  OR (NEW.runtime_role <> 'coordinator' AND NEW.work_scope_id IN (SELECT work_scope_id FROM coordinator_work_scope))
  OR (NEW.coordinator_head = 1 AND NEW.runtime_role <> 'coordinator')
  OR (NEW.coordinator_head = 1 AND NEW.continued_in_conv_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid conversation runtime role/work scope');
END;
CREATE TRIGGER coordinator_home_no_ordinary_owner_insert BEFORE INSERT ON product_conversation_work_scopes
WHEN NEW.work_scope_id IN (SELECT work_scope_id FROM coordinator_work_scope)
  OR EXISTS (SELECT 1 FROM product_conversations WHERE id = NEW.product_conversation_id AND kind = 'coordinator')
BEGIN
    SELECT RAISE(ABORT, 'Coordinator home is outside ordinary WorkScope ownership');
END;
CREATE TRIGGER coordinator_home_no_ordinary_owner_update BEFORE UPDATE ON product_conversation_work_scopes
WHEN NEW.work_scope_id IN (SELECT work_scope_id FROM coordinator_work_scope)
  OR EXISTS (SELECT 1 FROM product_conversations WHERE id = NEW.product_conversation_id AND kind = 'coordinator')
BEGIN
    SELECT RAISE(ABORT, 'Coordinator home is outside ordinary WorkScope ownership');
END;

CREATE TRIGGER coordinator_transcript_no_delete BEFORE DELETE ON conversations
WHEN OLD.runtime_role = 'coordinator'
BEGIN
    SELECT RAISE(ABORT, 'Coordinator transcript is outside ordinary Delete');
END;
CREATE TRIGGER coordinator_transcript_preserve_identity
BEFORE UPDATE OF runtime_role, archived, project_id, desired_base_branch ON conversations
WHEN OLD.runtime_role = 'coordinator'
  AND (NEW.runtime_role <> 'coordinator' OR NEW.archived <> 0
    OR OLD.project_id IS NOT NEW.project_id OR OLD.desired_base_branch IS NOT NEW.desired_base_branch)
BEGIN
    SELECT RAISE(ABORT, 'Coordinator transcript is outside ordinary project lifecycle');
END;
