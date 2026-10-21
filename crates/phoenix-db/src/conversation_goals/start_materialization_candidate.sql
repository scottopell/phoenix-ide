CREATE TABLE conversation_goal_start_materializations (
    start_id INTEGER NOT NULL PRIMARY KEY REFERENCES conversation_goal_starts(start_id) ON DELETE CASCADE,
    request_id TEXT NOT NULL UNIQUE REFERENCES conversation_goal_requests(request_id) ON DELETE CASCADE,
    effect_id INTEGER NOT NULL CHECK (typeof(effect_id)='integer' AND effect_id > 0),
    attempt_id INTEGER NOT NULL CHECK (typeof(attempt_id)='integer' AND attempt_id > 0),
    process_incarnation INTEGER NOT NULL CHECK (typeof(process_incarnation)='integer' AND process_incarnation >= 0),
    state_updated_at_us INTEGER NOT NULL CHECK (typeof(state_updated_at_us)='integer' AND state_updated_at_us > 0)
);

CREATE TRIGGER conversation_goal_start_materializations_append
BEFORE INSERT ON conversation_goal_start_materializations
WHEN EXISTS (SELECT 1 FROM conversation_goal_start_materializations
    WHERE start_id=NEW.start_id OR request_id=NEW.request_id OR rowid=NEW.rowid)
 OR NOT EXISTS (
    SELECT 1 FROM conversation_goal_starts s
    JOIN conversation_goal_start_canonical_inputs c ON c.start_id=s.start_id
    JOIN durable_turns t ON t.turn_id=s.live_turn_id
    JOIN conversation_goal_requests r ON r.request_id=NEW.request_id
    JOIN workflow_receipts wr ON wr.workflow_id=t.workflow_id
        AND wr.effect_id=NEW.effect_id AND wr.attempt_id=NEW.attempt_id
    JOIN workflow_transitions wt ON wt.workflow_id=t.workflow_id
        AND wt.transition_id=2 AND wt.generation=t.generation
    JOIN conversations cv ON cv.id=t.conversation_id AND cv.state_kind='llm_requesting'
    WHERE s.start_id=NEW.start_id AND t.goal_start_id=s.start_id
      AND t.input_kind='goal_start' AND t.materialized=1
      AND t.terminal_kind IS NULL AND t.owns_conversation=1
      AND wr.generation=t.generation AND wr.process_incarnation=NEW.process_incarnation
      AND wt.from_version=wr.declared_workflow_version AND wt.to_version=wt.from_version+1
      AND r.goal_id=s.goal_id AND r.owner_id=s.owner_id
      AND r.observed_version=s.observed_version AND r.cause='initial'
      AND r.historical_turn_id=s.historical_turn_id
      AND r.historical_workflow_id=s.historical_workflow_id
      AND r.live_turn_id=t.turn_id AND r.generation=t.generation)
BEGIN SELECT RAISE(ABORT, 'Start materialization must bind its canonical admitted request'); END;

CREATE TRIGGER conversation_goal_start_materializations_no_update
BEFORE UPDATE ON conversation_goal_start_materializations
BEGIN SELECT RAISE(ABORT, 'Start materialization witness is immutable'); END;

CREATE TRIGGER conversation_goal_start_materializations_no_delete
BEFORE DELETE ON conversation_goal_start_materializations
WHEN EXISTS (SELECT 1 FROM conversation_goal_starts s
    JOIN conversation_goal_ids g ON g.goal_id=s.goal_id AND g.owner_id=s.owner_id
    JOIN product_conversations p ON p.id=s.owner_id WHERE s.start_id=OLD.start_id)
BEGIN SELECT RAISE(ABORT, 'Start materialization witness is retained'); END;
