-- Preserve the landed mandatory route while admitting durable question waits.
PRAGMA defer_foreign_keys = ON;
CREATE TEMP TABLE question_wait_outbox_snapshot AS SELECT * FROM coordinator_watch_events;
DROP TRIGGER watch_event_accept_turn;
DROP TRIGGER watch_event_accept_steering;
DROP TABLE coordinator_watch_events;

CREATE TABLE coordinator_watch_events (
    event_id TEXT PRIMARY KEY NOT NULL CHECK(length(trim(event_id)) > 0),
    route_kind TEXT NOT NULL DEFAULT 'subscription'
        CHECK(route_kind IN ('subscription', 'mandatory_close_failure')),
    watch_id INTEGER REFERENCES coordinator_watches(id),
    mandatory_failure_occurrence_id TEXT UNIQUE,
    mandatory_source_product_id TEXT,
    source_occurrence_kind TEXT NOT NULL,
    source_occurrence_id TEXT NOT NULL CHECK(length(trim(source_occurrence_id)) > 0),
    source_generation INTEGER NOT NULL CHECK(source_generation >= 0),
    source_transcript_id TEXT NOT NULL,
    terminal_kind TEXT NOT NULL,
    terminal_reason TEXT,
    occurred_at_us INTEGER NOT NULL CHECK(typeof(occurred_at_us) = 'integer' AND occurred_at_us >= 0),
    continuation_state TEXT NOT NULL DEFAULT 'none'
        CHECK(continuation_state IN ('none', 'awaiting', 'suppressed')),
    delivery_state TEXT NOT NULL DEFAULT 'pending'
        CHECK(delivery_state IN ('pending', 'accepted', 'suppressed')),
    accepted_transcript_id TEXT,
    pending_failure_occurrence_id TEXT GENERATED ALWAYS AS (
        CASE WHEN delivery_state = 'pending' THEN mandatory_failure_occurrence_id END
    ) VIRTUAL REFERENCES close_cleanup_failures(failure_occurrence_id) ON DELETE RESTRICT,
    pending_source_product_id TEXT GENERATED ALWAYS AS (
        CASE WHEN delivery_state = 'pending' THEN mandatory_source_product_id END
    ) VIRTUAL REFERENCES product_conversations(id) ON DELETE RESTRICT,
    pending_source_transcript_id TEXT GENERATED ALWAYS AS (
        CASE WHEN route_kind = 'mandatory_close_failure' AND delivery_state = 'pending'
             THEN source_transcript_id END
    ) VIRTUAL REFERENCES conversations(id) ON DELETE RESTRICT,
    CHECK ((delivery_state = 'accepted') = (accepted_transcript_id IS NOT NULL)),
    UNIQUE(source_occurrence_kind, source_occurrence_id, source_generation, watch_id),
    CHECK (
        (route_kind = 'subscription'
         AND watch_id IS NOT NULL
         AND mandatory_failure_occurrence_id IS NULL
         AND mandatory_source_product_id IS NULL
         AND source_occurrence_kind IN ('direct_turn', 'creation', 'steering', 'wake', 'seeded_fork', 'interaction_response', 'continuation_summary', 'question_request', 'task_approval_wait')
         AND terminal_kind IN ('completed', 'failed', 'cancelled', 'awaiting_user_response', 'awaiting_task_approval')
         AND ((terminal_kind IN ('failed', 'awaiting_user_response', 'awaiting_task_approval')) = (terminal_reason IS NOT NULL))
         AND ((source_occurrence_kind = 'question_request') = (terminal_kind = 'awaiting_user_response'))
         AND (terminal_kind != 'awaiting_user_response' OR terminal_reason = 'question_request')
         AND ((source_occurrence_kind = 'task_approval_wait') = (terminal_kind = 'awaiting_task_approval'))
         AND (terminal_kind != 'awaiting_task_approval' OR terminal_reason = 'task_approval_wait'))
        OR
        (route_kind = 'mandatory_close_failure'
         AND watch_id IS NULL
         AND mandatory_failure_occurrence_id IS NOT NULL
         AND mandatory_source_product_id IS NOT NULL
         AND source_occurrence_kind = 'close_cleanup_failure'
         AND source_occurrence_id = mandatory_failure_occurrence_id
         AND event_id = mandatory_failure_occurrence_id
         AND source_generation = 0
         AND terminal_kind = 'cleanup_failed'
         AND terminal_reason IS NOT NULL
         AND continuation_state = 'none'
         AND delivery_state IN ('pending', 'accepted'))
    )
);
INSERT INTO coordinator_watch_events (
    event_id, route_kind, watch_id, mandatory_failure_occurrence_id, mandatory_source_product_id, source_occurrence_kind, source_occurrence_id,
    source_generation, source_transcript_id, terminal_kind, terminal_reason,
    occurred_at_us, continuation_state, delivery_state, accepted_transcript_id
)
SELECT event_id, route_kind, watch_id, mandatory_failure_occurrence_id, mandatory_source_product_id, source_occurrence_kind, source_occurrence_id,
       source_generation, source_transcript_id, terminal_kind, terminal_reason,
       occurred_at_us, continuation_state, delivery_state, accepted_transcript_id
FROM question_wait_outbox_snapshot;
DROP TABLE question_wait_outbox_snapshot;
CREATE INDEX coordinator_watch_events_pending ON coordinator_watch_events(delivery_state, occurred_at_us);

CREATE TRIGGER watch_event_mandatory_pending_reject_delete BEFORE DELETE ON coordinator_watch_events
WHEN OLD.route_kind = 'mandatory_close_failure' AND OLD.delivery_state = 'pending'
BEGIN SELECT RAISE(ABORT, 'pending mandatory Close event requires delivery'); END;

CREATE TRIGGER watch_event_payload_immutable BEFORE UPDATE ON coordinator_watch_events
WHEN NEW.event_id IS NOT OLD.event_id
  OR NEW.route_kind IS NOT OLD.route_kind
  OR NEW.watch_id IS NOT OLD.watch_id
  OR NEW.mandatory_failure_occurrence_id IS NOT OLD.mandatory_failure_occurrence_id
  OR NEW.mandatory_source_product_id IS NOT OLD.mandatory_source_product_id
  OR NEW.source_occurrence_kind IS NOT OLD.source_occurrence_kind
  OR NEW.source_occurrence_id IS NOT OLD.source_occurrence_id
  OR NEW.source_generation IS NOT OLD.source_generation
  OR NEW.source_transcript_id IS NOT OLD.source_transcript_id
  OR NEW.terminal_kind IS NOT OLD.terminal_kind
  OR NEW.terminal_reason IS NOT OLD.terminal_reason
  OR NEW.occurred_at_us IS NOT OLD.occurred_at_us
BEGIN SELECT RAISE(ABORT, 'watch event payload is immutable'); END;

CREATE TRIGGER watch_event_mandatory_source BEFORE INSERT ON coordinator_watch_events
WHEN NEW.route_kind = 'mandatory_close_failure' AND NOT EXISTS (
    SELECT 1 FROM close_cleanup_failures f
    JOIN product_conversations p ON p.id = f.source_product_conversation_id
    JOIN conversations root ON root.product_conversation_id = p.id
    WHERE f.failure_occurrence_id = NEW.mandatory_failure_occurrence_id
      AND NEW.event_id = f.failure_occurrence_id
      AND NEW.source_occurrence_id = f.failure_occurrence_id
      AND NEW.mandatory_source_product_id IS f.source_product_conversation_id
      AND NEW.terminal_reason IS f.reason
      AND NEW.occurred_at_us = f.occurred_at_us
      AND p.kind = 'ordinary'
      AND root.id = NEW.source_transcript_id AND root.runtime_role = 'user'
      AND root.parent_conversation_id IS NULL
      AND NOT EXISTS (SELECT 1 FROM conversations predecessor
                      WHERE predecessor.product_conversation_id = p.id
                        AND predecessor.continued_in_conv_id = root.id)
)
BEGIN SELECT RAISE(ABORT, 'mandatory Close event requires authoritative failure and product root'); END;

CREATE TRIGGER watch_event_accept_turn AFTER INSERT ON durable_turns
WHEN NEW.origin_subscription_event_id IS NOT NULL
BEGIN
    UPDATE coordinator_watch_events SET delivery_state = 'accepted', accepted_transcript_id = NEW.conversation_id
    WHERE event_id = NEW.origin_subscription_event_id AND delivery_state = 'pending'
      AND continuation_state = 'none'
      AND (route_kind = 'mandatory_close_failure' OR
           (route_kind = 'subscription' AND EXISTS (
                SELECT 1 FROM coordinator_watches w JOIN product_conversations p
                    ON p.id = w.source_product_conversation_id
                WHERE w.id = coordinator_watch_events.watch_id AND w.ended_at_us IS NULL
                  AND p.ordinary_lifecycle = 'open'
                  AND NOT EXISTS (SELECT 1 FROM close_obligations o
                                  WHERE o.product_conversation_id = p.id AND o.phase != 'completed'))))
      AND EXISTS (SELECT 1 FROM product_conversations p JOIN conversations c
                    ON c.product_conversation_id = p.id
                  WHERE c.id = NEW.conversation_id AND p.kind = 'coordinator'
                    AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
                    AND EXISTS (SELECT 1 FROM conversations head
                                WHERE head.product_conversation_id = p.id AND head.coordinator_head = 1));
    SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT, 'watch event no longer deliverable') END;
END;
CREATE TRIGGER watch_event_accept_steering AFTER INSERT ON steering_messages
WHEN NEW.origin_subscription_event_id IS NOT NULL
BEGIN
    UPDATE coordinator_watch_events SET delivery_state = 'accepted', accepted_transcript_id = NEW.conversation_id
    WHERE event_id = NEW.origin_subscription_event_id AND delivery_state = 'pending'
      AND continuation_state = 'none'
      AND (route_kind = 'mandatory_close_failure' OR
           (route_kind = 'subscription' AND EXISTS (
                SELECT 1 FROM coordinator_watches w JOIN product_conversations p
                    ON p.id = w.source_product_conversation_id
                WHERE w.id = coordinator_watch_events.watch_id AND w.ended_at_us IS NULL
                  AND p.ordinary_lifecycle = 'open'
                  AND NOT EXISTS (SELECT 1 FROM close_obligations o
                                  WHERE o.product_conversation_id = p.id AND o.phase != 'completed'))))
      AND EXISTS (SELECT 1 FROM product_conversations p JOIN conversations c
                    ON c.product_conversation_id = p.id
                  WHERE c.id = NEW.conversation_id AND p.kind = 'coordinator'
                    AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
                    AND EXISTS (SELECT 1 FROM conversations head
                                WHERE head.product_conversation_id = p.id AND head.coordinator_head = 1));
    SELECT CASE WHEN changes() != 1 THEN RAISE(ABORT, 'watch event no longer deliverable') END;
END;

DROP TRIGGER close_obligations_require_member_cleanup_before_delete;
CREATE TRIGGER close_obligations_require_member_cleanup_before_delete
BEFORE DELETE ON close_obligations
WHEN OLD.phase = 'completed'
  AND EXISTS (SELECT 1 FROM product_conversations WHERE id = OLD.product_conversation_id)
  AND (EXISTS (SELECT 1 FROM close_attempt_members WHERE attempt_id = OLD.attempt_id)
       OR EXISTS (SELECT 1 FROM close_attempt_scopes WHERE attempt_id = OLD.attempt_id))
BEGIN
    SELECT RAISE(ABORT, 'completed Close history must remove member snapshots before obligation deletion');
END;

-- A rebuilt parent can leave stale deferred-FK bookkeeping on older SQLite.
-- Validate the actual relationships before clearing that bookkeeping.
CREATE TEMP TABLE mandatory_close_outbox_fk_barrier (
    violation_count INTEGER NOT NULL CHECK(violation_count = 0)
);
INSERT INTO mandatory_close_outbox_fk_barrier
    SELECT COUNT(*) FROM pragma_foreign_key_check;
DROP TABLE mandatory_close_outbox_fk_barrier;
PRAGMA defer_foreign_keys = OFF;
CREATE TRIGGER close_retry_failure_event_requires_complete_pending_plan
BEFORE INSERT ON coordinator_watch_events
WHEN NEW.route_kind = 'mandatory_close_failure'
 AND EXISTS (SELECT 1 FROM close_cleanup_failures failure
    WHERE failure.failure_occurrence_id = NEW.mandatory_failure_occurrence_id
      AND failure.cleanup_run_ordinal > 1
      AND (SELECT COUNT(*) FROM close_cleanup_failure_resources child
          WHERE child.failure_occurrence_id = failure.failure_occurrence_id) <>
          (SELECT COUNT(*) FROM close_run_retry_effects effect
           WHERE effect.attempt_id = failure.attempt_id AND effect.run_ordinal = failure.cleanup_run_ordinal
             AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success
               WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal
                 AND success.ordinal = effect.ordinal)))
BEGIN
    SELECT RAISE(ABORT, 'retry failure must include every pending exact-run effect');
END;
