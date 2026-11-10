DROP VIEW conversation_goal_coordinator_history_mismatch;
DROP VIEW conversation_goal_coordinator_receipt_history;
DROP TRIGGER conversation_goal_coordinator_history_identity;
DROP TRIGGER conversation_goal_coordinator_receipt_history_identity;
DROP TRIGGER conversation_goal_report_fence;
DROP TRIGGER conversation_goal_user_request_existing_history;
DROP TRIGGER conversation_goal_user_command_history;
DROP TRIGGER conversation_goal_actor_source;
DROP TRIGGER conversation_goal_actors_no_update;
DROP TRIGGER conversation_goal_actors_no_delete;
DROP TRIGGER conversation_goal_preparations_source;
DROP TRIGGER conversation_goal_preparations_no_reinsert;
DROP TRIGGER conversation_goal_preparations_immutable;
DROP TRIGGER conversation_goal_preparations_no_delete;
DROP TRIGGER conversation_goal_coordinator_receipt_version;

CREATE TABLE conversation_goal_actors_new (
    actor_id INTEGER PRIMARY KEY,
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('user_api','owning_agent','coordinator','remote_coordinator','generated_host','user_control')),
    enrollment_id TEXT REFERENCES federation_enrollments(id) ON DELETE NO ACTION,
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
    CHECK ((kind = 'user_control' AND enrollment_id IS NULL AND source_product_id IS NULL AND transcript_id IS NULL
        AND workflow_id IS NULL AND turn_id IS NULL AND message_id IS NULL AND tool_use_id IS NULL AND host_action_id IS NULL
        AND control_principal_id IS NOT NULL AND typeof(control_principal_id) = 'blob' AND length(control_principal_id) = 16
        AND control_request_id IS NOT NULL AND typeof(control_request_id) = 'blob' AND length(control_request_id) = 16
        AND control_command IS NOT NULL AND control_command IN ('create','state')
        AND control_result_version IS NOT NULL AND typeof(control_result_version) = 'integer' AND control_result_version > 0)
      OR (kind <> 'user_control' AND control_principal_id IS NULL AND control_request_id IS NULL AND control_command IS NULL AND control_result_version IS NULL AND (
        (kind = 'user_api' AND enrollment_id IS NULL AND source_product_id IS NULL AND transcript_id IS NOT NULL
         AND workflow_id IS NULL AND turn_id IS NULL AND message_id IS NOT NULL
         AND tool_use_id IS NULL AND host_action_id IS NULL)
        OR (kind IN ('owning_agent','coordinator','remote_coordinator') AND source_product_id IS NOT NULL
         AND transcript_id IS NOT NULL AND workflow_id IS NOT NULL AND turn_id IS NOT NULL
         AND message_id IS NOT NULL AND tool_use_id IS NOT NULL AND trim(tool_use_id) <> ''
         AND host_action_id IS NULL AND ((kind = 'remote_coordinator' AND enrollment_id IS NOT NULL)
             OR (kind <> 'remote_coordinator' AND enrollment_id IS NULL)))
        OR (kind = 'generated_host' AND enrollment_id IS NULL AND source_product_id IS NULL AND transcript_id IS NULL
         AND workflow_id IS NULL AND turn_id IS NULL AND message_id IS NULL
         AND tool_use_id IS NULL AND host_action_id IS NOT NULL AND trim(host_action_id) <> '')
    )))
);
INSERT INTO conversation_goal_actors_new
  (actor_id,owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id,host_action_id,
   control_principal_id,control_request_id,control_command,control_result_version,enrollment_id)
SELECT actor_id,owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id,host_action_id,
       control_principal_id,control_request_id,control_command,control_result_version,NULL
FROM conversation_goal_actors;
DROP TABLE conversation_goal_actors;
ALTER TABLE conversation_goal_actors_new RENAME TO conversation_goal_actors;
CREATE UNIQUE INDEX conversation_goal_user_actor_request
ON conversation_goal_actors(owner_id,control_principal_id,control_request_id,control_command) WHERE kind = 'user_control';

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
CREATE TABLE conversation_goal_preparations_new (
    reference BLOB PRIMARY KEY NOT NULL CHECK(typeof(reference)='blob' AND length(reference)=16),
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL CHECK(source_kind IN ('local','remote')),
    enrollment_id TEXT REFERENCES federation_enrollments(id) ON DELETE NO ACTION,
    expires_at_us INTEGER CHECK(expires_at_us IS NULL OR (typeof(expires_at_us)='integer' AND expires_at_us>=0)),
    caller_conversation_id TEXT NOT NULL CHECK(length(trim(caller_conversation_id)) > 0),
    source_product_id TEXT NOT NULL CHECK(length(trim(source_product_id)) > 0),
    workflow_id INTEGER NOT NULL CHECK(typeof(workflow_id)='integer' AND workflow_id>0),
    turn_id INTEGER NOT NULL CHECK(typeof(turn_id)='integer' AND turn_id>0),
    message_id TEXT NOT NULL CHECK(length(trim(message_id)) > 0),
    tool_use_id TEXT NOT NULL CHECK(trim(tool_use_id)<>''),
    operation TEXT NOT NULL CHECK(operation IN ('create','replace','state')),
    expected_version INTEGER NOT NULL CHECK(typeof(expected_version)='integer' AND expected_version>=0),
    observed_goal_id TEXT,
    objective TEXT CHECK(objective IS NULL OR trim(objective)<>''),
    disposition TEXT CHECK(disposition IN ('underway','blocked','complete','exhausted')),
    reason TEXT CHECK(reason IS NULL OR trim(reason)<>''),
    apply_state TEXT NOT NULL DEFAULT 'prepared' CHECK(apply_state IN ('prepared','claimed')),
    CHECK ((source_kind='local' AND enrollment_id IS NULL AND expires_at_us IS NULL AND apply_state='prepared')
        OR (source_kind='remote' AND enrollment_id IS NOT NULL AND expires_at_us IS NOT NULL)),
    CHECK (
      (operation='create' AND expected_version=0 AND observed_goal_id IS NULL AND objective IS NOT NULL AND disposition IS NULL AND reason IS NULL)
      OR (operation='replace' AND expected_version>0 AND observed_goal_id IS NOT NULL AND objective IS NOT NULL AND disposition IS NULL AND reason IS NULL)
      OR (operation='state' AND expected_version>0 AND observed_goal_id IS NOT NULL AND objective IS NULL AND disposition IS NOT NULL AND ((disposition='underway' AND reason IS NULL) OR (disposition<>'underway' AND reason IS NOT NULL)))
    ),
    FOREIGN KEY(observed_goal_id,owner_id) REFERENCES conversation_goal_ids(goal_id,owner_id),
    UNIQUE(reference,owner_id)
);
INSERT INTO conversation_goal_preparations_new
  (reference,owner_id,source_kind,caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,
   operation,expected_version,observed_goal_id,objective,disposition,reason)
SELECT reference,owner_id,'local',caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,
       operation,expected_version,observed_goal_id,objective,disposition,reason
FROM conversation_goal_preparations;
DROP TABLE conversation_goal_preparations;
ALTER TABLE conversation_goal_preparations_new RENAME TO conversation_goal_preparations;
CREATE UNIQUE INDEX conversation_goal_local_preparations_source_call
ON conversation_goal_preparations(caller_conversation_id,message_id,tool_use_id)
WHERE source_kind='local';
CREATE UNIQUE INDEX conversation_goal_remote_preparations_source_call
ON conversation_goal_preparations(enrollment_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id)
WHERE source_kind='remote';

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
CREATE TRIGGER conversation_goal_actors_no_update BEFORE UPDATE ON conversation_goal_actors
BEGIN SELECT RAISE(ABORT, 'goal actor is immutable'); END;
CREATE TRIGGER conversation_goal_actors_no_delete BEFORE DELETE ON conversation_goal_actors
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal actor is retained'); END;
CREATE TRIGGER conversation_goal_user_actor_history_identity
AFTER INSERT ON conversation_goal_actors
WHEN EXISTS (SELECT 1 FROM conversation_goal_history h
    JOIN conversation_goal_user_requests r ON r.owner_id = h.owner_id AND r.result_version = h.version AND r.outcome = 'accepted'
    WHERE NEW.actor_id IN (h.requested_actor_id,h.applied_actor_id)
      AND (NEW.kind <> 'user_control' OR NEW.owner_id IS NOT r.owner_id
        OR NEW.control_principal_id IS NOT r.principal_id OR NEW.control_request_id IS NOT r.request_id
        OR NEW.control_command IS NOT r.command OR NEW.control_result_version IS NOT r.result_version))
BEGIN SELECT RAISE(ABORT, 'goal user attribution cannot be replaced by another source'); END;

CREATE TRIGGER conversation_goal_preparations_source
BEFORE INSERT ON conversation_goal_preparations
WHEN NEW.source_kind='local' AND NOT EXISTS (
    SELECT 1 FROM conversations c JOIN product_conversations p ON p.id=c.product_conversation_id
    JOIN durable_turns t ON t.conversation_id=c.id
    WHERE c.id=NEW.caller_conversation_id AND p.id=NEW.source_product_id AND p.kind='coordinator'
      AND t.turn_id=NEW.turn_id AND t.workflow_id=NEW.workflow_id
      AND (
        EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id=c.id
          AND m.message_id=NEW.message_id AND m.message_type='agent')
        OR (c.state_kind='tool_executing' AND t.disposition='Runtime'
          AND t.terminal_kind IS NULL AND t.owns_conversation=1)
      )
)
BEGIN SELECT RAISE(ABORT,'goal preparation source mismatch'); END;
CREATE TRIGGER conversation_goal_preparations_no_reinsert
BEFORE INSERT ON conversation_goal_preparations
WHEN EXISTS (SELECT 1 FROM conversation_goal_preparations p WHERE p.reference=NEW.reference OR p.rowid=NEW.rowid
    OR (p.source_kind='local' AND NEW.source_kind='local'
        AND p.caller_conversation_id=NEW.caller_conversation_id
        AND p.message_id=NEW.message_id AND p.tool_use_id=NEW.tool_use_id)
    OR (p.source_kind='remote' AND NEW.source_kind='remote'
        AND p.enrollment_id=NEW.enrollment_id
        AND p.source_product_id=NEW.source_product_id
        AND p.workflow_id=NEW.workflow_id AND p.turn_id=NEW.turn_id
        AND p.message_id=NEW.message_id AND p.tool_use_id=NEW.tool_use_id))
BEGIN SELECT RAISE(ABORT,'goal preparation cannot be replaced'); END;
CREATE TRIGGER conversation_goal_preparations_immutable
BEFORE UPDATE ON conversation_goal_preparations
WHEN NEW.reference IS NOT OLD.reference OR NEW.owner_id IS NOT OLD.owner_id
  OR NEW.source_kind IS NOT OLD.source_kind OR NEW.enrollment_id IS NOT OLD.enrollment_id
  OR NEW.expires_at_us IS NOT OLD.expires_at_us OR NEW.caller_conversation_id IS NOT OLD.caller_conversation_id
  OR NEW.source_product_id IS NOT OLD.source_product_id OR NEW.workflow_id IS NOT OLD.workflow_id
  OR NEW.turn_id IS NOT OLD.turn_id OR NEW.message_id IS NOT OLD.message_id OR NEW.tool_use_id IS NOT OLD.tool_use_id
  OR NEW.operation IS NOT OLD.operation OR NEW.expected_version IS NOT OLD.expected_version
  OR NEW.observed_goal_id IS NOT OLD.observed_goal_id OR NEW.objective IS NOT OLD.objective
  OR NEW.disposition IS NOT OLD.disposition OR NEW.reason IS NOT OLD.reason
  OR NOT (OLD.apply_state='prepared' AND NEW.apply_state='claimed')
BEGIN SELECT RAISE(ABORT,'goal preparation is immutable'); END;
CREATE TRIGGER conversation_goal_preparations_no_delete
BEFORE DELETE ON conversation_goal_preparations
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id=OLD.owner_id)
BEGIN SELECT RAISE(ABORT,'goal preparation is retained'); END;

CREATE TRIGGER conversation_goal_coordinator_receipt_version
BEFORE INSERT ON conversation_goal_coordinator_receipts
WHEN NOT EXISTS (SELECT 1 FROM conversation_goal_preparations p
    WHERE p.reference=NEW.preparation_reference AND p.owner_id=NEW.owner_id
      AND ((NEW.outcome='accepted' AND NEW.result_version=p.expected_version+1)
        OR (NEW.outcome='rejected_version' AND NEW.result_version<>p.expected_version)
        OR (NEW.outcome IN ('rejected_busy','rejected_transition') AND NEW.result_version=p.expected_version)))
BEGIN SELECT RAISE(ABORT,'coordinator goal receipt must match its preparation'); END;
CREATE TRIGGER conversation_goal_remote_receipts_require_claimed
BEFORE INSERT ON conversation_goal_coordinator_receipts
WHEN EXISTS (
    SELECT 1
    FROM conversation_goal_preparations p
    WHERE p.reference=NEW.preparation_reference
      AND p.source_kind='remote'
      AND p.apply_state!='claimed'
)
BEGIN SELECT RAISE(ABORT,'remote Goal receipt requires a claimed preparation'); END;

CREATE VIEW conversation_goal_coordinator_receipt_history AS
SELECT r.preparation_reference,r.owner_id,r.result_version,h.version,
    CASE p.operation WHEN 'state' THEN 'state' ELSE 'create' END AS command,
    p.expected_version,h.change_kind,h.recorded_at_us,r.recorded_at_us AS receipt_recorded_at_us,
    p.objective,g.objective AS history_objective,p.disposition,p.reason,h.state,h.reason AS history_reason,
    p.observed_goal_id,h.goal_id,
    p.source_kind,p.enrollment_id,e.caller_instance_id AS source_instance_id,p.source_product_id,p.caller_conversation_id,p.workflow_id,p.turn_id,p.message_id,p.tool_use_id,
    h.requested_actor_id,h.applied_actor_id
FROM conversation_goal_coordinator_receipts r
JOIN conversation_goal_preparations p ON p.reference=r.preparation_reference
LEFT JOIN federation_enrollments e ON e.id=p.enrollment_id
JOIN conversation_goal_history h ON h.owner_id=r.owner_id AND h.version=r.result_version
JOIN conversation_goal_ids g ON g.owner_id=h.owner_id AND g.goal_id=h.goal_id
WHERE r.outcome='accepted';
CREATE VIEW conversation_goal_coordinator_history_mismatch AS
SELECT v.preparation_reference,v.owner_id,v.result_version FROM conversation_goal_coordinator_receipt_history v
WHERE v.command<>v.change_kind OR v.recorded_at_us<>v.receipt_recorded_at_us
 OR (v.command='create' AND (v.objective<>v.history_objective OR v.state<>'underway'))
 OR (v.command='state' AND (v.observed_goal_id<>v.goal_id OR v.disposition<>v.state OR v.reason IS NOT v.history_reason))
 OR EXISTS (SELECT 1 FROM conversation_goal_actors a
    WHERE a.actor_id IN (v.requested_actor_id,v.applied_actor_id)
      AND (a.kind<>CASE v.source_kind WHEN 'local' THEN 'coordinator' ELSE 'remote_coordinator' END OR a.owner_id<>v.owner_id
        OR a.enrollment_id IS NOT v.enrollment_id
        OR (v.source_kind='remote' AND v.source_instance_id IS NULL)
        OR a.source_product_id IS NOT v.source_product_id OR a.transcript_id IS NOT v.caller_conversation_id
        OR a.workflow_id IS NOT v.workflow_id OR a.turn_id IS NOT v.turn_id
        OR a.message_id IS NOT v.message_id OR a.tool_use_id IS NOT v.tool_use_id));
CREATE TRIGGER conversation_goal_coordinator_history_identity
AFTER INSERT ON conversation_goal_history
WHEN EXISTS (SELECT 1 FROM conversation_goal_coordinator_history_mismatch WHERE owner_id=NEW.owner_id AND result_version=NEW.version)
BEGIN SELECT RAISE(ABORT,'goal history must match its coordinator preparation'); END;
CREATE TRIGGER conversation_goal_coordinator_receipt_history_identity
AFTER INSERT ON conversation_goal_coordinator_receipts
WHEN EXISTS (SELECT 1 FROM conversation_goal_coordinator_history_mismatch WHERE preparation_reference=NEW.preparation_reference)
BEGIN SELECT RAISE(ABORT,'coordinator receipt cannot relabel historical attribution'); END;
