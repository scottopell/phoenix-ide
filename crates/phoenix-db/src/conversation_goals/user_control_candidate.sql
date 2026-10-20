CREATE TABLE conversation_goal_user_requests (
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    principal_id BLOB NOT NULL CHECK (typeof(principal_id) = 'blob' AND length(principal_id) = 16),
    request_id BLOB NOT NULL CHECK (typeof(request_id) = 'blob' AND length(request_id) = 16),
    command TEXT NOT NULL CHECK (command IN ('create','state')),
    expected_version INTEGER NOT NULL CHECK (typeof(expected_version) = 'integer' AND expected_version >= 0),
    goal_id TEXT NOT NULL CHECK (trim(goal_id) <> ''),
    outcome TEXT NOT NULL CHECK (outcome IN ('accepted','rejected_version','rejected_busy','rejected_transition')),
    result_version INTEGER NOT NULL CHECK (typeof(result_version) = 'integer' AND result_version >= 0),
    rejected_objective TEXT,
    rejected_state TEXT,
    rejected_reason TEXT,
    recorded_at_us INTEGER NOT NULL CHECK (typeof(recorded_at_us) = 'integer' AND recorded_at_us >= 0),
    snapshot_version INTEGER GENERATED ALWAYS AS (CASE WHEN result_version > 0 THEN result_version END) VIRTUAL,
    accepted_goal_id TEXT GENERATED ALWAYS AS (CASE WHEN outcome = 'accepted' THEN goal_id END) VIRTUAL,
    PRIMARY KEY (owner_id,principal_id,request_id),
    UNIQUE (owner_id,principal_id,request_id,outcome,command,result_version),
    FOREIGN KEY (owner_id,snapshot_version) REFERENCES conversation_goal_history(owner_id,version) DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (owner_id,snapshot_version,accepted_goal_id) REFERENCES conversation_goal_history(owner_id,version,goal_id) DEFERRABLE INITIALLY DEFERRED,
    CHECK ((outcome = 'accepted' AND result_version = expected_version + 1
                AND rejected_objective IS NULL AND rejected_state IS NULL AND rejected_reason IS NULL)
        OR (outcome <> 'accepted' AND rejected_state IS NOT NULL AND rejected_state IN ('underway','blocked','suspended','complete','exhausted')
                AND ((rejected_state = 'underway' AND rejected_reason IS NULL)
                    OR (rejected_state <> 'underway' AND rejected_reason IS NOT NULL AND trim(rejected_reason) <> ''))
                AND ((command = 'create' AND rejected_state = 'underway' AND rejected_objective IS NOT NULL AND trim(rejected_objective) <> '')
                    OR (command = 'state' AND rejected_objective IS NULL)))),
    CHECK (outcome <> 'rejected_version' OR result_version <> expected_version)
);

CREATE UNIQUE INDEX conversation_goal_user_request_accepted_version
ON conversation_goal_user_requests(owner_id,result_version) WHERE outcome = 'accepted';

CREATE TRIGGER conversation_goal_user_requests_no_reinsert
BEFORE INSERT ON conversation_goal_user_requests
WHEN EXISTS (SELECT 1 FROM conversation_goal_user_requests r
    WHERE r.owner_id = NEW.owner_id
      AND ((r.principal_id = NEW.principal_id AND r.request_id = NEW.request_id)
        OR (r.outcome = 'accepted' AND NEW.outcome = 'accepted' AND r.result_version = NEW.result_version)))
BEGIN SELECT RAISE(ABORT, 'goal user request is retained without replacement'); END;

CREATE TRIGGER conversation_goal_user_requests_no_update BEFORE UPDATE ON conversation_goal_user_requests
BEGIN SELECT RAISE(ABORT, 'goal user request is immutable'); END;
CREATE TRIGGER conversation_goal_user_requests_no_delete BEFORE DELETE ON conversation_goal_user_requests
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal user request is retained'); END;

PRAGMA defer_foreign_keys = ON;
CREATE TEMP TABLE goal_user_actor_stage AS
SELECT actor_id,owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id,host_action_id
FROM conversation_goal_actors;
CREATE TEMP TABLE goal_user_history_stage AS
SELECT owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us
FROM conversation_goal_history;
CREATE TEMP TABLE goal_user_binding_stage AS
SELECT turn_id,workflow_id,owner_id,observed_version,goal_id FROM conversation_goal_bindings;

DROP TRIGGER conversation_goal_history_no_delete;
DROP TRIGGER conversation_goal_bindings_no_delete;
DROP TRIGGER conversation_goal_history_append;
DROP TRIGGER conversation_goal_history_transition;
DROP TRIGGER conversation_goal_report_fence;
DROP TRIGGER conversation_goal_binding_admission;
DELETE FROM conversation_goal_bindings;
DELETE FROM conversation_goal_history;
DROP TABLE conversation_goal_actors;

CREATE UNIQUE INDEX conversation_goal_history_actor_pair
ON conversation_goal_history(owner_id,version,requested_actor_id,applied_actor_id);

CREATE TABLE conversation_goal_actors (
    actor_id INTEGER PRIMARY KEY,
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('user_api','owning_agent','coordinator','generated_host','user_control')),
    source_product_id TEXT,
    transcript_id TEXT,
    workflow_id INTEGER,
    turn_id INTEGER,
    message_id TEXT,
    tool_use_id TEXT,
    host_action_id TEXT,
    control_principal_id BLOB,
    control_request_id BLOB,
    control_command TEXT,
    control_result_version INTEGER,
    control_outcome TEXT GENERATED ALWAYS AS (CASE WHEN kind = 'user_control' THEN 'accepted' END) VIRTUAL,
    control_applied_actor_id INTEGER GENERATED ALWAYS AS (CASE WHEN kind = 'user_control' THEN actor_id END) VIRTUAL,
    UNIQUE (actor_id, owner_id),
    FOREIGN KEY (owner_id,control_principal_id,control_request_id,control_outcome,control_command,control_result_version)
        REFERENCES conversation_goal_user_requests(owner_id,principal_id,request_id,outcome,command,result_version),
    FOREIGN KEY (owner_id,control_result_version,actor_id,control_applied_actor_id)
        REFERENCES conversation_goal_history(owner_id,version,requested_actor_id,applied_actor_id) DEFERRABLE INITIALLY DEFERRED,
    CHECK ((kind = 'user_control' AND source_product_id IS NULL AND transcript_id IS NULL
        AND workflow_id IS NULL AND turn_id IS NULL AND message_id IS NULL AND tool_use_id IS NULL AND host_action_id IS NULL
        AND control_principal_id IS NOT NULL AND typeof(control_principal_id) = 'blob' AND length(control_principal_id) = 16
        AND control_request_id IS NOT NULL AND typeof(control_request_id) = 'blob' AND length(control_request_id) = 16
        AND control_command IS NOT NULL AND control_command IN ('create','state')
        AND control_result_version IS NOT NULL AND typeof(control_result_version) = 'integer' AND control_result_version > 0)
      OR (kind <> 'user_control' AND control_principal_id IS NULL AND control_request_id IS NULL AND control_command IS NULL AND control_result_version IS NULL AND (
        (kind = 'user_api' AND source_product_id IS NULL AND transcript_id IS NOT NULL
         AND workflow_id IS NULL AND turn_id IS NULL AND message_id IS NOT NULL
         AND tool_use_id IS NULL AND host_action_id IS NULL)
        OR (kind IN ('owning_agent','coordinator') AND source_product_id IS NOT NULL
         AND transcript_id IS NOT NULL AND workflow_id IS NOT NULL AND turn_id IS NOT NULL
         AND message_id IS NOT NULL AND tool_use_id IS NOT NULL AND trim(tool_use_id) <> ''
         AND host_action_id IS NULL)
        OR (kind = 'generated_host' AND source_product_id IS NULL AND transcript_id IS NULL
         AND workflow_id IS NULL AND turn_id IS NULL AND message_id IS NULL
         AND tool_use_id IS NULL AND host_action_id IS NOT NULL AND trim(host_action_id) <> '')
    )))
);

CREATE UNIQUE INDEX conversation_goal_user_actor_request
ON conversation_goal_actors(owner_id,control_principal_id,control_request_id,control_command) WHERE kind = 'user_control';

INSERT INTO conversation_goal_actors (actor_id,owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id,host_action_id)
SELECT actor_id,owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id,host_action_id FROM goal_user_actor_stage;
INSERT INTO conversation_goal_history (owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us)
SELECT owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us FROM goal_user_history_stage;
INSERT INTO conversation_goal_bindings (turn_id,workflow_id,owner_id,observed_version,goal_id)
SELECT turn_id,workflow_id,owner_id,observed_version,goal_id FROM goal_user_binding_stage;
DROP TABLE goal_user_actor_stage;
DROP TABLE goal_user_history_stage;
DROP TABLE goal_user_binding_stage;

CREATE TRIGGER conversation_goal_actor_source
BEFORE INSERT ON conversation_goal_actors
WHEN (NEW.kind = 'user_api' AND NOT EXISTS (
        SELECT 1 FROM messages m JOIN conversations c ON c.id = m.conversation_id
        WHERE m.conversation_id = NEW.transcript_id AND m.message_id = NEW.message_id
          AND m.origin_kind = 'user_api' AND c.product_conversation_id = NEW.owner_id))
 OR (NEW.kind IN ('owning_agent','coordinator') AND NOT EXISTS (
        SELECT 1 FROM durable_turns t JOIN conversations c ON c.id = t.conversation_id
        JOIN product_conversations p ON p.id = c.product_conversation_id
        JOIN messages m ON m.conversation_id = c.id AND m.message_id = NEW.message_id AND m.message_type = 'agent'
        WHERE t.turn_id = NEW.turn_id AND t.workflow_id = NEW.workflow_id
          AND c.id = NEW.transcript_id AND p.id = NEW.source_product_id
          AND ((NEW.kind = 'owning_agent' AND p.id = NEW.owner_id AND p.kind = 'ordinary')
            OR (NEW.kind = 'coordinator' AND p.kind = 'coordinator'))))
BEGIN SELECT RAISE(ABORT, 'goal actor source mismatch'); END;
CREATE TRIGGER conversation_goal_history_append
BEFORE INSERT ON conversation_goal_history
WHEN NEW.version <> COALESCE((SELECT MAX(version) + 1 FROM conversation_goal_history WHERE owner_id = NEW.owner_id), 1)
BEGIN SELECT RAISE(ABORT, 'goal version must append'); END;

CREATE TRIGGER conversation_goal_history_transition
BEFORE INSERT ON conversation_goal_history
WHEN (NEW.change_kind = 'create' AND (
        NEW.state <> 'underway' OR EXISTS (
            SELECT 1 FROM conversation_goal_history WHERE goal_id = NEW.goal_id)))
 OR (NEW.change_kind <> 'create' AND NOT EXISTS (
        SELECT 1 FROM conversation_goal_history prior
        WHERE prior.owner_id = NEW.owner_id AND prior.version = NEW.version - 1
          AND prior.goal_id = NEW.goal_id AND prior.state NOT IN ('complete','exhausted','suspended')
          AND (NEW.state <> 'underway' OR prior.state = 'blocked')))
BEGIN SELECT RAISE(ABORT, 'goal status transition or supersession mismatch'); END;
CREATE TRIGGER conversation_goal_binding_admission
BEFORE INSERT ON conversation_goal_bindings
WHEN NOT EXISTS (
    SELECT 1 FROM durable_turns t JOIN conversations c ON c.id = t.conversation_id
    JOIN product_conversations p ON p.id = c.product_conversation_id
    WHERE t.turn_id = NEW.turn_id AND t.workflow_id = NEW.workflow_id
      AND p.id = NEW.owner_id AND p.kind = 'ordinary'
      AND t.disposition = 'Runtime' AND t.terminal_kind IS NULL
      AND t.canonical_message_id IS NULL AND t.owns_conversation = 1)
 OR COALESCE(NEW.observed_version, 0) <> COALESCE((
    SELECT MAX(version) FROM conversation_goal_history WHERE owner_id = NEW.owner_id), 0)
 OR (NEW.goal_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM conversation_goal_history WHERE owner_id = NEW.owner_id
    AND version = NEW.observed_version AND goal_id = NEW.goal_id
    AND state = 'underway'))
 OR (NEW.goal_id IS NULL AND EXISTS (
    SELECT 1 FROM conversation_goal_history WHERE owner_id = NEW.owner_id
    AND version = NEW.observed_version AND state = 'underway'))
BEGIN SELECT RAISE(ABORT, 'goal binding must match admission observation'); END;

CREATE TRIGGER conversation_goal_report_fence
BEFORE INSERT ON conversation_goal_history
WHEN NEW.change_kind = 'report' AND NOT EXISTS (
    SELECT 1 FROM conversation_goal_bindings b
    JOIN conversation_goal_history admitted ON admitted.owner_id = b.owner_id AND admitted.version = b.observed_version
    JOIN conversation_goal_actors actor ON actor.actor_id = NEW.requested_actor_id
    WHERE b.turn_id = NEW.report_turn_id AND b.owner_id = NEW.owner_id
      AND b.goal_id = NEW.goal_id
      AND b.observed_version = NEW.version - 1 AND admitted.state = 'underway'
      AND actor.kind = 'owning_agent' AND actor.owner_id = b.owner_id
      AND actor.turn_id = b.turn_id AND actor.workflow_id = b.workflow_id)
BEGIN SELECT RAISE(ABORT, 'goal report does not match admitted fence'); END;
CREATE TRIGGER conversation_goal_history_no_delete BEFORE DELETE ON conversation_goal_history
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal history is retained'); END;
CREATE TRIGGER conversation_goal_actors_no_update BEFORE UPDATE ON conversation_goal_actors
BEGIN SELECT RAISE(ABORT, 'goal attribution is immutable'); END;
CREATE TRIGGER conversation_goal_actors_no_delete BEFORE DELETE ON conversation_goal_actors
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal attribution is retained'); END;
CREATE TRIGGER conversation_goal_bindings_no_delete BEFORE DELETE ON conversation_goal_bindings
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal admission binding is retained'); END;

CREATE TRIGGER conversation_goal_user_request_owner
BEFORE INSERT ON conversation_goal_user_requests
WHEN NOT EXISTS (SELECT 1 FROM product_conversations WHERE id = NEW.owner_id AND kind = 'ordinary')
BEGIN SELECT RAISE(ABORT, 'goal user request owner must be ordinary'); END;

CREATE TRIGGER conversation_goal_user_request_existing_history
BEFORE INSERT ON conversation_goal_user_requests
WHEN NEW.outcome = 'accepted' AND EXISTS (
    SELECT 1 FROM conversation_goal_history h WHERE h.owner_id = NEW.owner_id AND h.version = NEW.result_version
      AND (h.goal_id <> NEW.goal_id OR h.change_kind <> NEW.command
        OR NOT EXISTS (SELECT 1 FROM conversation_goal_actors a WHERE a.actor_id = h.requested_actor_id
            AND a.owner_id = NEW.owner_id AND a.kind = 'user_control'
            AND a.control_principal_id = NEW.principal_id AND a.control_request_id = NEW.request_id
            AND a.control_command = NEW.command)
        OR NOT EXISTS (SELECT 1 FROM conversation_goal_actors a WHERE a.actor_id = h.applied_actor_id
            AND a.owner_id = NEW.owner_id AND a.kind = 'user_control'
            AND a.control_principal_id = NEW.principal_id AND a.control_request_id = NEW.request_id
            AND a.control_command = NEW.command)))
BEGIN SELECT RAISE(ABORT, 'accepted goal request cannot relabel historical attribution'); END;

CREATE TRIGGER conversation_goal_user_command_history
BEFORE INSERT ON conversation_goal_history
WHEN EXISTS (
    SELECT 1 FROM conversation_goal_actors a
    WHERE a.actor_id IN (NEW.requested_actor_id,NEW.applied_actor_id) AND a.kind = 'user_control'
      AND NOT EXISTS (SELECT 1 FROM conversation_goal_user_requests r
        WHERE r.owner_id = NEW.owner_id AND r.principal_id = a.control_principal_id
          AND r.request_id = a.control_request_id AND r.command = a.control_command
          AND r.command = NEW.change_kind AND r.outcome = 'accepted'
          AND r.expected_version = NEW.version - 1 AND r.result_version = NEW.version AND r.goal_id = NEW.goal_id))
 OR EXISTS (
    SELECT 1 FROM conversation_goal_user_requests r
    WHERE r.owner_id = NEW.owner_id AND r.result_version = NEW.version AND r.outcome = 'accepted'
      AND (r.command <> NEW.change_kind OR r.goal_id <> NEW.goal_id
        OR NOT EXISTS (SELECT 1 FROM conversation_goal_actors a WHERE a.actor_id = NEW.requested_actor_id
            AND a.owner_id = NEW.owner_id AND a.kind = 'user_control'
            AND a.control_principal_id = r.principal_id AND a.control_request_id = r.request_id
            AND a.control_command = r.command)
        OR NOT EXISTS (SELECT 1 FROM conversation_goal_actors a WHERE a.actor_id = NEW.applied_actor_id
            AND a.owner_id = NEW.owner_id AND a.kind = 'user_control'
            AND a.control_principal_id = r.principal_id AND a.control_request_id = r.request_id
            AND a.control_command = r.command)))
BEGIN SELECT RAISE(ABORT, 'goal history must match its accepted user command'); END;

CREATE TRIGGER conversation_goal_user_actor_history_identity
AFTER INSERT ON conversation_goal_actors
WHEN EXISTS (SELECT 1 FROM conversation_goal_history h
    JOIN conversation_goal_user_requests r ON r.owner_id = h.owner_id AND r.result_version = h.version AND r.outcome = 'accepted'
    WHERE NEW.actor_id IN (h.requested_actor_id,h.applied_actor_id)
      AND (NEW.kind <> 'user_control' OR NEW.owner_id IS NOT r.owner_id
        OR NEW.control_principal_id IS NOT r.principal_id OR NEW.control_request_id IS NOT r.request_id
        OR NEW.control_command IS NOT r.command OR NEW.control_result_version IS NOT r.result_version))
BEGIN SELECT RAISE(ABORT, 'goal user attribution cannot be replaced by another source'); END;
