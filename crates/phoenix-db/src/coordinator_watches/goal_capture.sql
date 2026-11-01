ALTER TABLE coordinator_watch_events ADD COLUMN goal_capture_kind TEXT NOT NULL DEFAULT 'legacy'
    CHECK(goal_capture_kind IN ('legacy','disabled','captured'));
CREATE TABLE coordinator_watch_goal_captures (
    event_id TEXT PRIMARY KEY NOT NULL REFERENCES coordinator_watch_events(event_id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('no_goal','saved_goal')),
    owner_id TEXT NOT NULL CHECK(length(trim(owner_id)) > 0),
    owner_version INTEGER NOT NULL CHECK(typeof(owner_version) = 'integer' AND owner_version >= 0),
    goal_id TEXT,
    objective TEXT,
    state TEXT,
    reason TEXT,
    usage_kind TEXT NOT NULL CHECK(usage_kind = 'unavailable_not_captured'),
    CHECK((kind = 'no_goal' AND goal_id IS NULL AND objective IS NULL AND state IS NULL AND reason IS NULL)
       OR (kind = 'saved_goal' AND owner_version > 0
           AND goal_id IS NOT NULL AND length(trim(goal_id)) > 0
           AND objective IS NOT NULL AND length(trim(objective)) > 0
           AND state IS NOT NULL AND ((state = 'underway' AND reason IS NULL)
             OR (state IN ('blocked','complete','exhausted','suspended') AND reason IS NOT NULL AND length(trim(reason)) > 0))))
);
ALTER TABLE coordinator_watch_events ADD COLUMN goal_capture_event_id TEXT GENERATED ALWAYS AS (
    CASE WHEN goal_capture_kind = 'captured' THEN event_id END
) VIRTUAL REFERENCES coordinator_watch_goal_captures(event_id) DEFERRABLE INITIALLY DEFERRED;
CREATE TRIGGER watch_goal_capture_marker_immutable BEFORE UPDATE OF goal_capture_kind ON coordinator_watch_events
WHEN NEW.goal_capture_kind IS NOT OLD.goal_capture_kind
BEGIN SELECT RAISE(ABORT, 'watch goal capture marker is immutable'); END;
CREATE TRIGGER watch_goal_capture_local_only BEFORE INSERT ON coordinator_watch_events
WHEN (NEW.route_kind != 'subscription' AND NEW.goal_capture_kind != 'legacy')
  OR (NEW.source_occurrence_kind = 'goal_change' AND NEW.goal_capture_kind != 'captured')
BEGIN SELECT RAISE(ABORT, 'goal capture requires eligible local subscription'); END;
CREATE TRIGGER watch_goal_capture_source BEFORE INSERT ON coordinator_watch_goal_captures
WHEN NOT EXISTS (SELECT 1 FROM coordinator_watch_events e JOIN coordinator_watches w ON w.id=e.watch_id
    WHERE e.event_id=NEW.event_id AND e.route_kind='subscription'
      AND e.goal_capture_kind='captured' AND w.source_product_conversation_id=NEW.owner_id
      AND (e.source_occurrence_kind != 'goal_change' OR
           (NEW.kind='saved_goal' AND e.source_occurrence_id=NEW.owner_id AND e.source_generation=NEW.owner_version)))
BEGIN SELECT RAISE(ABORT, 'watch goal capture differs from event owner or kind'); END;
CREATE TRIGGER watch_goal_capture_immutable BEFORE UPDATE ON coordinator_watch_goal_captures
BEGIN SELECT RAISE(ABORT, 'watch goal capture is immutable'); END;
CREATE TRIGGER watch_goal_capture_retained BEFORE DELETE ON coordinator_watch_goal_captures
WHEN EXISTS (SELECT 1 FROM coordinator_watch_events WHERE event_id=OLD.event_id)
BEGIN SELECT RAISE(ABORT, 'watch goal capture follows event lifetime'); END;
