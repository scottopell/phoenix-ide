-- Migration134 control tables; schema presence does not install runtime controls.
CREATE TABLE conversation_goal_control_requests (
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    principal_id BLOB NOT NULL CHECK (typeof(principal_id) = 'blob' AND length(principal_id) = 16),
    request_id BLOB NOT NULL CHECK (typeof(request_id) = 'blob' AND length(request_id) = 16),
    command TEXT NOT NULL CHECK (command IN ('stop','release')),
    expected_control_version INTEGER NOT NULL CHECK (typeof(expected_control_version) = 'integer' AND expected_control_version >= 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('accepted','rejected')),
    rejection TEXT,
    result_control_version INTEGER NOT NULL CHECK (typeof(result_control_version) = 'integer' AND result_control_version >= 0),
    result_stopped INTEGER,
    recorded_at_us INTEGER NOT NULL CHECK (typeof(recorded_at_us) = 'integer' AND recorded_at_us >= 0),
    PRIMARY KEY (owner_id, principal_id, request_id),
    UNIQUE (owner_id, principal_id, request_id, outcome, result_control_version, result_stopped),
    CHECK ((result_control_version = 0 AND result_stopped IS NULL)
        OR (result_control_version > 0 AND result_stopped IN (0,1) AND result_stopped IS NOT NULL)),
    CHECK ((outcome = 'accepted' AND rejection IS NULL
            AND result_control_version = expected_control_version + 1
            AND result_stopped = CASE command WHEN 'stop' THEN 1 ELSE 0 END)
        OR (outcome = 'rejected' AND rejection IS NOT NULL AND rejection = 'control_version_conflict'
            AND result_control_version <> expected_control_version))
);

CREATE UNIQUE INDEX conversation_goal_control_accepted_version
ON conversation_goal_control_requests(owner_id, result_control_version)
WHERE outcome = 'accepted';

CREATE TABLE conversation_goal_controls (
    owner_id TEXT NOT NULL PRIMARY KEY REFERENCES product_conversations(id) ON DELETE CASCADE,
    control_version INTEGER NOT NULL CHECK (typeof(control_version) = 'integer' AND control_version > 0),
    stopped INTEGER NOT NULL CHECK (stopped IN (0,1)),
    principal_id BLOB NOT NULL,
    request_id BLOB NOT NULL,
    receipt_outcome TEXT NOT NULL DEFAULT 'accepted' CHECK (receipt_outcome = 'accepted'),
    FOREIGN KEY (owner_id, principal_id, request_id, receipt_outcome, control_version, stopped)
        REFERENCES conversation_goal_control_requests(owner_id, principal_id, request_id, outcome, result_control_version, result_stopped)
);

CREATE TABLE conversation_goal_control_bindings (
    turn_id INTEGER PRIMARY KEY CHECK (typeof(turn_id) = 'integer' AND turn_id > 0),
    workflow_id INTEGER NOT NULL CHECK (typeof(workflow_id) = 'integer' AND workflow_id > 0),
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    control_version INTEGER NOT NULL CHECK (typeof(control_version) = 'integer' AND control_version > 0),
    principal_id BLOB NOT NULL,
    request_id BLOB NOT NULL,
    receipt_outcome TEXT NOT NULL DEFAULT 'accepted' CHECK (receipt_outcome = 'accepted'),
    admitted_stopped INTEGER NOT NULL DEFAULT 0 CHECK (admitted_stopped = 0),
    FOREIGN KEY (owner_id, principal_id, request_id, receipt_outcome, control_version, admitted_stopped)
        REFERENCES conversation_goal_control_requests(owner_id, principal_id, request_id, outcome, result_control_version, result_stopped)
);

CREATE TRIGGER conversation_goal_control_request_source
BEFORE INSERT ON conversation_goal_control_requests
WHEN NOT EXISTS (SELECT 1 FROM product_conversations p WHERE p.id = NEW.owner_id
    AND p.kind = 'ordinary' AND p.ordinary_lifecycle = 'open'
    AND NOT EXISTS (SELECT 1 FROM close_obligations o WHERE o.product_conversation_id = p.id AND o.phase <> 'completed'))
 OR NEW.result_control_version <> COALESCE((SELECT control_version FROM conversation_goal_controls WHERE owner_id = NEW.owner_id), 0)
    + CASE NEW.outcome WHEN 'accepted' THEN 1 ELSE 0 END
 OR (NEW.outcome = 'rejected' AND NEW.result_stopped IS NOT
        (SELECT stopped FROM conversation_goal_controls WHERE owner_id = NEW.owner_id))
BEGIN SELECT RAISE(ABORT, 'control receipt must match ordinary owner state'); END;

CREATE TRIGGER conversation_goal_control_apply_accepted
AFTER INSERT ON conversation_goal_control_requests
WHEN NEW.outcome = 'accepted'
BEGIN
    INSERT INTO conversation_goal_controls (owner_id, control_version, stopped, principal_id, request_id)
    VALUES (NEW.owner_id, NEW.result_control_version, NEW.result_stopped, NEW.principal_id, NEW.request_id)
    ON CONFLICT(owner_id) DO UPDATE SET control_version = excluded.control_version,
        stopped = excluded.stopped, principal_id = excluded.principal_id, request_id = excluded.request_id;
END;

CREATE TRIGGER conversation_goal_control_version
BEFORE UPDATE ON conversation_goal_controls
WHEN NEW.owner_id <> OLD.owner_id OR NEW.control_version <> OLD.control_version + 1
BEGIN SELECT RAISE(ABORT, 'control version must advance'); END;

CREATE TRIGGER conversation_goal_control_binding_admission
BEFORE INSERT ON conversation_goal_control_bindings
WHEN NOT EXISTS (
    SELECT 1 FROM conversation_goal_controls ctl
    JOIN product_conversations p ON p.id = ctl.owner_id AND p.kind = 'ordinary' AND p.ordinary_lifecycle = 'open'
    JOIN durable_turns t ON t.turn_id = NEW.turn_id AND t.workflow_id = NEW.workflow_id
    JOIN conversations c ON c.id = t.conversation_id
    WHERE ctl.owner_id = NEW.owner_id AND c.product_conversation_id = NEW.owner_id
      AND ctl.control_version = NEW.control_version AND ctl.stopped = 0
      AND ctl.principal_id = NEW.principal_id AND ctl.request_id = NEW.request_id
      AND NOT EXISTS (SELECT 1 FROM close_obligations o WHERE o.product_conversation_id = p.id AND o.phase <> 'completed')
      AND t.disposition = 'Runtime' AND t.terminal_kind IS NULL
      AND t.owns_conversation = 1 AND t.canonical_message_id IS NULL)
BEGIN SELECT RAISE(ABORT, 'control binding must match open admission'); END;

CREATE TRIGGER conversation_goal_control_requests_no_reinsert
BEFORE INSERT ON conversation_goal_control_requests
WHEN EXISTS (SELECT 1 FROM conversation_goal_control_requests r WHERE r.owner_id = NEW.owner_id
    AND ((r.principal_id = NEW.principal_id AND r.request_id = NEW.request_id)
        OR (r.outcome = 'accepted' AND NEW.outcome = 'accepted' AND r.result_control_version = NEW.result_control_version)))
BEGIN SELECT RAISE(ABORT, 'control request is retained without replacement'); END;

CREATE TRIGGER conversation_goal_control_bindings_no_reinsert
BEFORE INSERT ON conversation_goal_control_bindings
WHEN EXISTS (SELECT 1 FROM conversation_goal_control_bindings WHERE turn_id = NEW.turn_id)
BEGIN SELECT RAISE(ABORT, 'control admission cannot be recaptured'); END;

CREATE TRIGGER conversation_goal_controls_no_replace
BEFORE INSERT ON conversation_goal_controls
WHEN EXISTS (SELECT 1 FROM conversation_goal_controls old WHERE old.owner_id = NEW.owner_id
    AND (NEW.control_version <> old.control_version + 1 OR NOT EXISTS (
        SELECT 1 FROM conversation_goal_control_requests r WHERE r.owner_id = NEW.owner_id
          AND r.principal_id = NEW.principal_id AND r.request_id = NEW.request_id
          AND r.outcome = 'accepted' AND r.expected_control_version = old.control_version
          AND r.result_control_version = NEW.control_version AND r.result_stopped = NEW.stopped)))
BEGIN SELECT RAISE(ABORT, 'control state cannot be replaced by historical evidence'); END;

CREATE TRIGGER conversation_goal_control_requests_no_update BEFORE UPDATE ON conversation_goal_control_requests
BEGIN SELECT RAISE(ABORT, 'control request receipt is immutable'); END;
CREATE TRIGGER conversation_goal_control_requests_no_delete BEFORE DELETE ON conversation_goal_control_requests
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'control request receipt is retained'); END;
CREATE TRIGGER conversation_goal_controls_no_delete BEFORE DELETE ON conversation_goal_controls
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'control state is retained'); END;
CREATE TRIGGER conversation_goal_control_bindings_no_update BEFORE UPDATE ON conversation_goal_control_bindings
BEGIN SELECT RAISE(ABORT, 'control admission is immutable'); END;
CREATE TRIGGER conversation_goal_control_bindings_no_delete BEFORE DELETE ON conversation_goal_control_bindings
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'control admission is retained'); END;
