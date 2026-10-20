CREATE TABLE conversation_goal_limits (
    goal_id TEXT NOT NULL PRIMARY KEY REFERENCES conversation_goal_ids(goal_id) ON DELETE CASCADE,
    request_limit INTEGER NOT NULL CHECK (typeof(request_limit) = 'integer' AND request_limit > 0)
);

CREATE TRIGGER conversation_goal_limits_create_only
BEFORE INSERT ON conversation_goal_limits
WHEN EXISTS (SELECT 1 FROM conversation_goal_history WHERE goal_id = NEW.goal_id)
 OR EXISTS (SELECT 1 FROM conversation_goal_limits WHERE goal_id = NEW.goal_id OR rowid = NEW.rowid)
BEGIN SELECT RAISE(ABORT, 'goal limit belongs to genuine create'); END;

CREATE TRIGGER conversation_goal_limits_no_update BEFORE UPDATE ON conversation_goal_limits
BEGIN SELECT RAISE(ABORT, 'goal limit is immutable'); END;

CREATE TRIGGER conversation_goal_limits_no_delete BEFORE DELETE ON conversation_goal_limits
WHEN EXISTS (SELECT 1 FROM conversation_goal_ids g JOIN product_conversations p ON p.id = g.owner_id WHERE g.goal_id = OLD.goal_id)
BEGIN SELECT RAISE(ABORT, 'goal limit is retained'); END;

CREATE TABLE conversation_goal_requests (
    request_id TEXT NOT NULL PRIMARY KEY CHECK (trim(request_id) <> ''),
    goal_id TEXT NOT NULL REFERENCES conversation_goal_limits(goal_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (typeof(ordinal) = 'integer' AND ordinal > 0),
    owner_id TEXT NOT NULL,
    observed_version INTEGER NOT NULL CHECK (typeof(observed_version) = 'integer' AND observed_version > 0),
    historical_workflow_id INTEGER NOT NULL CHECK (typeof(historical_workflow_id) = 'integer' AND historical_workflow_id > 0),
    historical_turn_id INTEGER NOT NULL CHECK (typeof(historical_turn_id) = 'integer' AND historical_turn_id > 0),
    generation INTEGER NOT NULL CHECK (typeof(generation) = 'integer' AND generation >= 0),
    cause TEXT NOT NULL CHECK (cause IN ('initial','after_checkpoint','summary')),
    predecessor_request_id TEXT,
    summary_operation TEXT,
    live_turn_id INTEGER REFERENCES durable_turns(turn_id) ON DELETE SET NULL,
    purpose TEXT GENERATED ALWAYS AS (CASE WHEN cause = 'summary' THEN 'summary' ELSE 'normal' END) VIRTUAL,
    UNIQUE (goal_id, ordinal),
    UNIQUE (request_id, goal_id),
    FOREIGN KEY (owner_id, observed_version, goal_id)
        REFERENCES conversation_goal_history(owner_id, version, goal_id) DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (predecessor_request_id, goal_id)
        REFERENCES conversation_goal_requests(request_id, goal_id) DEFERRABLE INITIALLY DEFERRED,
    CHECK ((cause = 'initial' AND predecessor_request_id IS NULL AND summary_operation IS NULL)
        OR (cause = 'after_checkpoint' AND predecessor_request_id IS NOT NULL AND summary_operation IS NULL)
        OR (cause = 'summary' AND predecessor_request_id IS NULL AND summary_operation IS NOT NULL AND trim(summary_operation) <> ''))
);

CREATE UNIQUE INDEX conversation_goal_requests_initial_cause
    ON conversation_goal_requests(goal_id, historical_turn_id) WHERE cause = 'initial';
CREATE UNIQUE INDEX conversation_goal_requests_checkpoint_cause
    ON conversation_goal_requests(predecessor_request_id) WHERE cause = 'after_checkpoint';
CREATE UNIQUE INDEX conversation_goal_requests_summary_cause
    ON conversation_goal_requests(goal_id, summary_operation) WHERE cause = 'summary';
CREATE INDEX conversation_goal_requests_live_turn ON conversation_goal_requests(live_turn_id) WHERE live_turn_id IS NOT NULL;

CREATE TABLE conversation_goal_request_checkpoints (
    request_id TEXT NOT NULL PRIMARY KEY REFERENCES conversation_goal_requests(request_id) ON DELETE CASCADE,
    checkpoint_key TEXT NOT NULL CHECK (trim(checkpoint_key) <> '')
);

CREATE TRIGGER conversation_goal_requests_append
BEFORE INSERT ON conversation_goal_requests
WHEN EXISTS (SELECT 1 FROM conversation_goal_requests WHERE request_id = NEW.request_id OR rowid = NEW.rowid)
 OR NOT EXISTS (SELECT 1 FROM conversation_goal_limits l WHERE l.goal_id = NEW.goal_id AND NEW.ordinal <= l.request_limit)
 OR COALESCE((SELECT MAX(ordinal) FROM conversation_goal_requests WHERE goal_id = NEW.goal_id), 0) = 9223372036854775807
 OR NEW.ordinal - 1 <> COALESCE((SELECT MAX(ordinal) FROM conversation_goal_requests WHERE goal_id = NEW.goal_id), 0)
 OR NOT EXISTS (
    SELECT 1 FROM conversation_goal_history h
    WHERE h.owner_id = NEW.owner_id AND h.goal_id = NEW.goal_id AND h.state_kind = 'underway'
      AND NOT EXISTS (SELECT 1 FROM conversation_goal_history later WHERE later.owner_id = h.owner_id AND later.version > h.version))
 OR NOT EXISTS (
    SELECT 1 FROM durable_turns t JOIN conversation_goal_bindings b ON b.turn_id = t.turn_id
    WHERE t.turn_id = NEW.live_turn_id AND t.turn_id = NEW.historical_turn_id
      AND t.workflow_id = NEW.historical_workflow_id AND t.generation = NEW.generation
      AND t.disposition = 'Runtime' AND t.terminal_kind IS NULL AND t.owns_conversation = 1
      AND b.workflow_id = t.workflow_id AND b.owner_id = NEW.owner_id
      AND b.observed_version = NEW.observed_version AND b.goal_id = NEW.goal_id)
 OR (NEW.cause = 'after_checkpoint' AND NOT EXISTS (
    SELECT 1 FROM conversation_goal_requests r JOIN conversation_goal_request_checkpoints c USING (request_id)
    WHERE r.request_id = NEW.predecessor_request_id AND r.goal_id = NEW.goal_id AND r.ordinal < NEW.ordinal))
BEGIN SELECT RAISE(ABORT, 'goal request must append within capacity at its bound execution'); END;

CREATE TRIGGER conversation_goal_requests_detach_only
BEFORE UPDATE ON conversation_goal_requests
WHEN NEW.request_id IS NOT OLD.request_id OR NEW.goal_id IS NOT OLD.goal_id
 OR NEW.ordinal IS NOT OLD.ordinal OR NEW.owner_id IS NOT OLD.owner_id
 OR NEW.observed_version IS NOT OLD.observed_version
 OR NEW.historical_workflow_id IS NOT OLD.historical_workflow_id
 OR NEW.historical_turn_id IS NOT OLD.historical_turn_id OR NEW.generation IS NOT OLD.generation
 OR NEW.cause IS NOT OLD.cause OR NEW.predecessor_request_id IS NOT OLD.predecessor_request_id
 OR NEW.summary_operation IS NOT OLD.summary_operation OR NEW.rowid IS NOT OLD.rowid
 OR (NEW.live_turn_id IS NOT OLD.live_turn_id AND NOT (OLD.live_turn_id IS NOT NULL AND NEW.live_turn_id IS NULL))
BEGIN SELECT RAISE(ABORT, 'goal request permits only live detachment'); END;

CREATE TRIGGER conversation_goal_requests_no_delete BEFORE DELETE ON conversation_goal_requests
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal request charge is retained'); END;

CREATE TRIGGER conversation_goal_request_checkpoints_no_replace
BEFORE INSERT ON conversation_goal_request_checkpoints
WHEN EXISTS (SELECT 1 FROM conversation_goal_request_checkpoints WHERE request_id = NEW.request_id OR rowid = NEW.rowid)
 OR NOT EXISTS (
    SELECT 1 FROM conversation_goal_requests r JOIN durable_turns t ON t.turn_id = r.live_turn_id
    JOIN conversation_goal_bindings b ON b.turn_id = t.turn_id
    WHERE r.request_id = NEW.request_id AND t.workflow_id = r.historical_workflow_id
      AND t.disposition = 'Runtime'
      AND t.terminal_kind IS NULL AND t.owns_conversation = 1
      AND b.workflow_id = t.workflow_id AND b.owner_id = r.owner_id
      AND b.observed_version = r.observed_version AND b.goal_id = r.goal_id)
BEGIN SELECT RAISE(ABORT, 'goal request checkpoint requires live uncheckpointed intent'); END;

CREATE TRIGGER conversation_goal_request_checkpoints_no_update BEFORE UPDATE ON conversation_goal_request_checkpoints
BEGIN SELECT RAISE(ABORT, 'goal request checkpoint is immutable'); END;

CREATE TRIGGER conversation_goal_request_checkpoints_no_delete BEFORE DELETE ON conversation_goal_request_checkpoints
WHEN EXISTS (SELECT 1 FROM conversation_goal_requests r JOIN product_conversations p ON p.id = r.owner_id WHERE r.request_id = OLD.request_id)
BEGIN SELECT RAISE(ABORT, 'goal request checkpoint is retained'); END;
