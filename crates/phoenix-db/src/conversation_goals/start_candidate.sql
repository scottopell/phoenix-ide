CREATE TABLE conversation_goal_starts (
    start_id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (start_id > 0),
    owner_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    request_id TEXT NOT NULL CHECK (trim(request_id) <> ''),
    goal_id TEXT NOT NULL,
    observed_version INTEGER NOT NULL CHECK (typeof(observed_version) = 'integer' AND observed_version > 0),
    requester_kind TEXT NOT NULL CHECK (requester_kind IN ('owner','coordinator')),
    owner_principal_id TEXT,
    source_owner_id TEXT,
    source_workflow_id INTEGER,
    source_turn_id INTEGER,
    source_transcript_id TEXT,
    source_message_id TEXT,
    source_call_id TEXT,
    historical_workflow_id INTEGER NOT NULL CHECK (typeof(historical_workflow_id) = 'integer' AND historical_workflow_id > 0),
    historical_turn_id INTEGER NOT NULL UNIQUE CHECK (typeof(historical_turn_id) = 'integer' AND historical_turn_id > 0),
    historical_transcript_id TEXT NOT NULL CHECK (trim(historical_transcript_id) <> ''),
    live_turn_id INTEGER REFERENCES durable_turns(turn_id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
    UNIQUE (owner_id, request_id),
    FOREIGN KEY (owner_id,observed_version,goal_id) REFERENCES conversation_goal_history(owner_id,version,goal_id) DEFERRABLE INITIALLY DEFERRED,
    CHECK ((requester_kind='owner' AND owner_principal_id IS NOT NULL AND trim(owner_principal_id) <> ''
            AND source_owner_id IS NULL AND source_workflow_id IS NULL AND source_turn_id IS NULL
            AND source_transcript_id IS NULL AND source_message_id IS NULL AND source_call_id IS NULL)
        OR (requester_kind='coordinator' AND owner_principal_id IS NULL
            AND source_owner_id IS NOT NULL AND trim(source_owner_id) <> ''
            AND source_workflow_id IS NOT NULL AND typeof(source_workflow_id)='integer' AND source_workflow_id > 0
            AND source_turn_id IS NOT NULL AND typeof(source_turn_id)='integer' AND source_turn_id > 0
            AND source_transcript_id IS NOT NULL AND trim(source_transcript_id) <> ''
            AND source_message_id IS NOT NULL AND trim(source_message_id) <> ''
            AND source_call_id IS NOT NULL AND trim(source_call_id) <> '')),
    CHECK (live_turn_id IS NULL OR live_turn_id = historical_turn_id)
);

CREATE TRIGGER conversation_goal_starts_no_replace BEFORE INSERT ON conversation_goal_starts
WHEN NEW.live_turn_id IS NULL OR EXISTS (SELECT 1 FROM conversation_goal_starts
             WHERE start_id=NEW.start_id OR (owner_id=NEW.owner_id AND request_id=NEW.request_id)
                OR historical_turn_id=NEW.historical_turn_id)
BEGIN SELECT RAISE(ABORT, 'Start acceptance identity is retained'); END;

CREATE TRIGGER conversation_goal_starts_detach_only BEFORE UPDATE ON conversation_goal_starts
WHEN NEW.start_id IS NOT OLD.start_id OR NEW.owner_id IS NOT OLD.owner_id
 OR NEW.request_id IS NOT OLD.request_id OR NEW.goal_id IS NOT OLD.goal_id OR NEW.observed_version IS NOT OLD.observed_version
 OR NEW.requester_kind IS NOT OLD.requester_kind OR NEW.owner_principal_id IS NOT OLD.owner_principal_id
 OR NEW.source_owner_id IS NOT OLD.source_owner_id OR NEW.source_transcript_id IS NOT OLD.source_transcript_id
 OR NEW.source_message_id IS NOT OLD.source_message_id OR NEW.source_call_id IS NOT OLD.source_call_id
 OR NEW.source_workflow_id IS NOT OLD.source_workflow_id OR NEW.source_turn_id IS NOT OLD.source_turn_id
 OR NEW.historical_workflow_id IS NOT OLD.historical_workflow_id OR NEW.historical_turn_id IS NOT OLD.historical_turn_id
 OR NEW.historical_transcript_id IS NOT OLD.historical_transcript_id
 OR (NEW.live_turn_id IS NOT OLD.live_turn_id AND NOT (OLD.live_turn_id IS NOT NULL AND NEW.live_turn_id IS NULL))
BEGIN SELECT RAISE(ABORT, 'Start acceptance permits only live detachment'); END;

CREATE TRIGGER conversation_goal_starts_no_delete BEFORE DELETE ON conversation_goal_starts
WHEN EXISTS (SELECT 1 FROM product_conversations WHERE id=OLD.owner_id)
BEGIN SELECT RAISE(ABORT, 'Start acceptance is retained'); END;

ALTER TABLE durable_turns ADD COLUMN input_kind TEXT NOT NULL DEFAULT 'message' CHECK (input_kind IN ('message','goal_start'));
ALTER TABLE durable_turns ADD COLUMN goal_start_id INTEGER REFERENCES conversation_goal_starts(start_id) DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE durable_turns ADD COLUMN materialized INTEGER NOT NULL DEFAULT 0 CHECK (typeof(materialized)='integer' AND materialized IN (0,1));
UPDATE durable_turns SET materialized=1 WHERE canonical_message_id IS NOT NULL;

CREATE TRIGGER durable_turns_input_insert BEFORE INSERT ON durable_turns
WHEN (NEW.input_kind='message' AND (NEW.goal_start_id IS NOT NULL OR NEW.prepared_payload IS NULL OR NEW.origin_kind IS NULL
      OR ((NEW.materialized=1) != (NEW.canonical_message_id IS NOT NULL))))
 OR (NEW.input_kind='goal_start' AND (NEW.goal_start_id IS NULL OR NEW.prepared_payload IS NOT NULL
      OR NEW.origin_kind IS NOT NULL OR NEW.origin_product_conversation_id IS NOT NULL OR NEW.origin_transcript_id IS NOT NULL
      OR NEW.canonical_message_id IS NOT NULL OR NEW.materialized <> 0 OR NEW.disposition <> 'Runtime' OR NOT EXISTS (
          SELECT 1 FROM conversation_goal_starts s WHERE s.start_id=NEW.goal_start_id
          AND s.historical_turn_id=NEW.turn_id AND s.historical_workflow_id=NEW.workflow_id
          AND s.historical_transcript_id=NEW.conversation_id AND s.live_turn_id=NEW.turn_id)))
BEGIN SELECT RAISE(ABORT, 'canonical input kind or materialization is invalid'); END;

CREATE TRIGGER durable_turns_input_update BEFORE UPDATE ON durable_turns
WHEN NEW.input_kind IS NOT OLD.input_kind OR NEW.goal_start_id IS NOT OLD.goal_start_id OR NEW.materialized < OLD.materialized
 OR (NEW.input_kind='message' AND (NEW.goal_start_id IS NOT NULL OR NEW.prepared_payload IS NULL OR NEW.origin_kind IS NULL
      OR ((NEW.materialized=1) != (NEW.canonical_message_id IS NOT NULL))))
 OR (NEW.input_kind='goal_start' AND (NEW.prepared_payload IS NOT NULL OR NEW.origin_kind IS NOT NULL
      OR NEW.origin_product_conversation_id IS NOT NULL OR NEW.origin_transcript_id IS NOT NULL
      OR NEW.canonical_message_id IS NOT NULL OR NEW.disposition <> 'Runtime'))
BEGIN SELECT RAISE(ABORT, 'canonical input kind or materialization cannot change'); END;

DROP INDEX durable_turns_discoverable_nonterminal;
CREATE INDEX durable_turns_discoverable_nonterminal ON durable_turns(turn_id,workflow_id)
    WHERE disposition='Runtime' AND terminal_kind IS NULL AND materialized=0;
