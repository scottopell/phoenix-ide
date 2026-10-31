CREATE TABLE conversation_goal_preparations (
    reference BLOB PRIMARY KEY NOT NULL CHECK(typeof(reference)='blob' AND length(reference)=16),
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
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
    CHECK (
      (operation='create' AND expected_version=0 AND observed_goal_id IS NULL AND objective IS NOT NULL AND disposition IS NULL AND reason IS NULL)
      OR (operation='replace' AND expected_version>0 AND observed_goal_id IS NOT NULL AND objective IS NOT NULL AND disposition IS NULL AND reason IS NULL)
      OR (operation='state' AND expected_version>0 AND observed_goal_id IS NOT NULL AND objective IS NULL AND disposition IS NOT NULL AND ((disposition='underway' AND reason IS NULL) OR (disposition<>'underway' AND reason IS NOT NULL)))
    ),
    FOREIGN KEY(observed_goal_id,owner_id) REFERENCES conversation_goal_ids(goal_id,owner_id),
    UNIQUE(reference,owner_id),
    UNIQUE(caller_conversation_id,message_id,tool_use_id)
);
CREATE TRIGGER conversation_goal_preparations_source
BEFORE INSERT ON conversation_goal_preparations
WHEN NOT EXISTS (
    SELECT 1 FROM conversations c JOIN product_conversations p ON p.id=c.product_conversation_id
    JOIN durable_turns t ON t.conversation_id=c.id
    JOIN messages m ON m.conversation_id=c.id
    WHERE c.id=NEW.caller_conversation_id AND p.id=NEW.source_product_id AND p.kind='coordinator'
      AND t.turn_id=NEW.turn_id AND t.workflow_id=NEW.workflow_id
      AND m.message_id=NEW.message_id AND m.message_type='agent'
)
BEGIN SELECT RAISE(ABORT,'goal preparation source mismatch'); END;
CREATE TRIGGER conversation_goal_preparations_no_reinsert
BEFORE INSERT ON conversation_goal_preparations
WHEN EXISTS (SELECT 1 FROM conversation_goal_preparations p WHERE p.reference=NEW.reference OR p.rowid=NEW.rowid
    OR (p.caller_conversation_id=NEW.caller_conversation_id AND p.message_id=NEW.message_id AND p.tool_use_id=NEW.tool_use_id))
BEGIN SELECT RAISE(ABORT,'goal preparation cannot be replaced'); END;
CREATE TRIGGER conversation_goal_preparations_immutable
BEFORE UPDATE ON conversation_goal_preparations
BEGIN SELECT RAISE(ABORT,'goal preparation is immutable'); END;
CREATE TRIGGER conversation_goal_preparations_no_delete
BEFORE DELETE ON conversation_goal_preparations
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id=OLD.owner_id)
BEGIN SELECT RAISE(ABORT,'goal preparation is retained'); END;

CREATE TABLE conversation_goal_coordinator_receipts (
    preparation_reference BLOB PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    outcome TEXT NOT NULL CHECK(outcome IN ('accepted','rejected_version','rejected_busy','rejected_transition')),
    result_version INTEGER NOT NULL CHECK(typeof(result_version)='integer' AND result_version>=0),
    recorded_at_us INTEGER NOT NULL CHECK(typeof(recorded_at_us)='integer' AND recorded_at_us>=0),
    snapshot_version INTEGER GENERATED ALWAYS AS (CASE WHEN result_version>0 THEN result_version END) VIRTUAL,
    FOREIGN KEY(preparation_reference,owner_id) REFERENCES conversation_goal_preparations(reference,owner_id) ON DELETE CASCADE,
    FOREIGN KEY(owner_id,snapshot_version) REFERENCES conversation_goal_history(owner_id,version) DEFERRABLE INITIALLY DEFERRED
);
CREATE UNIQUE INDEX conversation_goal_coordinator_accepted_version
ON conversation_goal_coordinator_receipts(owner_id,result_version) WHERE outcome='accepted';
CREATE TRIGGER conversation_goal_coordinator_receipt_version
BEFORE INSERT ON conversation_goal_coordinator_receipts
WHEN NOT EXISTS (SELECT 1 FROM conversation_goal_preparations p
    WHERE p.reference=NEW.preparation_reference AND p.owner_id=NEW.owner_id
      AND ((NEW.outcome='accepted' AND NEW.result_version=p.expected_version+1)
        OR (NEW.outcome='rejected_version' AND NEW.result_version<>p.expected_version)
        OR (NEW.outcome IN ('rejected_busy','rejected_transition') AND NEW.result_version=p.expected_version)))
BEGIN SELECT RAISE(ABORT,'coordinator goal receipt must match its preparation'); END;
CREATE VIEW conversation_goal_coordinator_receipt_history AS
SELECT r.preparation_reference,r.owner_id,r.result_version,h.version,
    CASE p.operation WHEN 'state' THEN 'state' ELSE 'create' END AS command,
    p.expected_version,h.change_kind,h.recorded_at_us,r.recorded_at_us AS receipt_recorded_at_us,
    p.objective,g.objective AS history_objective,p.disposition,p.reason,h.state,h.reason AS history_reason,
    p.observed_goal_id,h.goal_id,
    p.source_product_id,p.caller_conversation_id,p.workflow_id,p.turn_id,p.message_id,p.tool_use_id,
    h.requested_actor_id,h.applied_actor_id
FROM conversation_goal_coordinator_receipts r
JOIN conversation_goal_preparations p ON p.reference=r.preparation_reference
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
      AND (a.kind<>'coordinator' OR a.owner_id<>v.owner_id
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
CREATE TRIGGER conversation_goal_coordinator_receipts_no_reinsert
BEFORE INSERT ON conversation_goal_coordinator_receipts
WHEN EXISTS (SELECT 1 FROM conversation_goal_coordinator_receipts WHERE preparation_reference=NEW.preparation_reference OR rowid=NEW.rowid)
BEGIN SELECT RAISE(ABORT,'coordinator goal receipt cannot be replaced'); END;
CREATE TRIGGER conversation_goal_coordinator_receipts_immutable
BEFORE UPDATE ON conversation_goal_coordinator_receipts
BEGIN SELECT RAISE(ABORT,'coordinator goal receipt is immutable'); END;
CREATE TRIGGER conversation_goal_coordinator_receipts_no_delete
BEFORE DELETE ON conversation_goal_coordinator_receipts
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id=OLD.owner_id)
BEGIN SELECT RAISE(ABORT,'coordinator goal receipt is retained'); END;
