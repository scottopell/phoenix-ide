CREATE TABLE conversation_goal_preparations (
    reference BLOB PRIMARY KEY NOT NULL CHECK(typeof(reference)='blob' AND length(reference)=16),
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    caller_conversation_id TEXT NOT NULL REFERENCES conversations(id),
    source_product_id TEXT NOT NULL REFERENCES product_conversations(id),
    workflow_id INTEGER NOT NULL CHECK(typeof(workflow_id)='integer'),
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id),
    message_id TEXT NOT NULL REFERENCES messages(message_id),
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
