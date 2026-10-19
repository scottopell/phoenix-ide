CREATE TABLE conversation_goal_ids (
    goal_id TEXT PRIMARY KEY CHECK (trim(goal_id) <> ''),
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    UNIQUE (goal_id, owner_id)
);

CREATE TRIGGER conversation_goal_owner_is_ordinary
BEFORE INSERT ON conversation_goal_ids
WHEN NOT EXISTS (SELECT 1 FROM product_conversations WHERE id = NEW.owner_id AND kind = 'ordinary')
BEGIN SELECT RAISE(ABORT, 'goal owner must be ordinary'); END;

CREATE TABLE conversation_goal_actors (
    actor_id INTEGER PRIMARY KEY,
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('user_api','owning_agent','coordinator','generated_host')),
    source_product_id TEXT,
    transcript_id TEXT,
    workflow_id INTEGER,
    turn_id INTEGER,
    message_id TEXT,
    tool_use_id TEXT,
    host_action_id TEXT,
    UNIQUE (actor_id, owner_id),
    CHECK (
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
    )
);

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

CREATE TABLE conversation_goal_history (
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    version INTEGER NOT NULL CHECK (typeof(version) = 'integer' AND version > 0),
    goal_id TEXT NOT NULL,
    objective_revision INTEGER NOT NULL CHECK (typeof(objective_revision) = 'integer' AND objective_revision > 0),
    objective TEXT NOT NULL CHECK (trim(objective) <> ''),
    state TEXT NOT NULL CHECK (state IN ('active','blocked','suspended','complete','cleared')),
    reason TEXT,
    action_id TEXT NOT NULL CHECK (trim(action_id) <> ''),
    change_kind TEXT NOT NULL CHECK (change_kind IN ('create','edit','state','report')),
    report_turn_id INTEGER REFERENCES conversation_goal_bindings(turn_id),
    requested_actor_id INTEGER NOT NULL,
    applied_actor_id INTEGER NOT NULL,
    recorded_at_us INTEGER NOT NULL CHECK (typeof(recorded_at_us) = 'integer' AND recorded_at_us >= 0),
    PRIMARY KEY (owner_id, version),
    UNIQUE (owner_id, action_id),
    UNIQUE (owner_id, version, goal_id, objective_revision),
    FOREIGN KEY (goal_id, owner_id) REFERENCES conversation_goal_ids(goal_id, owner_id),
    FOREIGN KEY (requested_actor_id, owner_id) REFERENCES conversation_goal_actors(actor_id, owner_id),
    FOREIGN KEY (applied_actor_id, owner_id) REFERENCES conversation_goal_actors(actor_id, owner_id),
    CHECK ((state = 'active' AND reason IS NULL)
        OR (state <> 'active' AND reason IS NOT NULL AND trim(reason) <> '')),
    CHECK ((change_kind = 'report' AND report_turn_id IS NOT NULL)
        OR (change_kind <> 'report' AND report_turn_id IS NULL))
);

CREATE TRIGGER conversation_goal_history_append
BEFORE INSERT ON conversation_goal_history
WHEN NEW.version <> COALESCE((SELECT MAX(version) + 1 FROM conversation_goal_history WHERE owner_id = NEW.owner_id), 1)
BEGIN SELECT RAISE(ABORT, 'goal version must append'); END;

CREATE TABLE conversation_goal_bindings (
    turn_id INTEGER PRIMARY KEY,
    workflow_id INTEGER NOT NULL,
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    observed_version INTEGER,
    goal_id TEXT,
    objective_revision INTEGER,
    FOREIGN KEY (owner_id, observed_version) REFERENCES conversation_goal_history(owner_id, version),
    FOREIGN KEY (owner_id, observed_version, goal_id, objective_revision)
        REFERENCES conversation_goal_history(owner_id, version, goal_id, objective_revision),
    CHECK ((goal_id IS NULL AND objective_revision IS NULL)
        OR (goal_id IS NOT NULL AND objective_revision IS NOT NULL AND observed_version IS NOT NULL))
);

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
    AND objective_revision = NEW.objective_revision AND state = 'active'))
 OR (NEW.goal_id IS NULL AND EXISTS (
    SELECT 1 FROM conversation_goal_history WHERE owner_id = NEW.owner_id
    AND version = NEW.observed_version AND state = 'active'))
BEGIN SELECT RAISE(ABORT, 'goal binding must match admission observation'); END;

CREATE TRIGGER conversation_goal_report_fence
BEFORE INSERT ON conversation_goal_history
WHEN NEW.change_kind = 'report' AND NOT EXISTS (
    SELECT 1 FROM conversation_goal_bindings b
    JOIN conversation_goal_history admitted ON admitted.owner_id = b.owner_id AND admitted.version = b.observed_version
    JOIN conversation_goal_actors actor ON actor.actor_id = NEW.requested_actor_id
    WHERE b.turn_id = NEW.report_turn_id AND b.owner_id = NEW.owner_id
      AND b.goal_id = NEW.goal_id AND b.objective_revision = NEW.objective_revision
      AND b.observed_version = NEW.version - 1 AND admitted.state = 'active'
      AND actor.kind = 'owning_agent' AND actor.owner_id = b.owner_id
      AND actor.turn_id = b.turn_id AND actor.workflow_id = b.workflow_id)
BEGIN SELECT RAISE(ABORT, 'goal report does not match admitted fence'); END;

CREATE TRIGGER conversation_goal_ids_no_update BEFORE UPDATE ON conversation_goal_ids
BEGIN SELECT RAISE(ABORT, 'goal identity is immutable'); END;
CREATE TRIGGER conversation_goal_ids_no_delete BEFORE DELETE ON conversation_goal_ids
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal identity is retained'); END;
CREATE TRIGGER conversation_goal_history_no_update BEFORE UPDATE ON conversation_goal_history
BEGIN SELECT RAISE(ABORT, 'goal history is immutable'); END;
CREATE TRIGGER conversation_goal_history_no_delete BEFORE DELETE ON conversation_goal_history
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal history is retained'); END;
CREATE TRIGGER conversation_goal_actors_no_update BEFORE UPDATE ON conversation_goal_actors
BEGIN SELECT RAISE(ABORT, 'goal attribution is immutable'); END;
CREATE TRIGGER conversation_goal_actors_no_delete BEFORE DELETE ON conversation_goal_actors
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal attribution is retained'); END;
CREATE TRIGGER conversation_goal_bindings_no_update BEFORE UPDATE ON conversation_goal_bindings
BEGIN SELECT RAISE(ABORT, 'goal admission binding is immutable'); END;
CREATE TRIGGER conversation_goal_bindings_no_delete BEFORE DELETE ON conversation_goal_bindings
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'goal admission binding is retained'); END;
