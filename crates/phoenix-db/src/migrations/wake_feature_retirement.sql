CREATE TABLE workflow_feature_retirements (
    workflow_id INTEGER PRIMARY KEY REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    reason TEXT NOT NULL CHECK (reason = 'WakeFeatureRetired')
) STRICT;

INSERT INTO workflow_feature_retirements (workflow_id, reason)
SELECT workflow_id, 'WakeFeatureRetired'
FROM workflows
WHERE profile_kind = 'wake';

CREATE TABLE wake_occurrence_feature_retirements (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (reason = 'WakeFeatureRetired'),
    PRIMARY KEY (conversation_id, message_id),
    FOREIGN KEY (conversation_id, message_id)
        REFERENCES messages(conversation_id, message_id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

INSERT INTO wake_occurrence_feature_retirements (conversation_id, message_id, reason)
SELECT conversation_id, message_id, 'WakeFeatureRetired'
FROM steering_execution_occurrences
WHERE source_kind = 'wake';

CREATE TABLE coordinator_event_feature_retirements (
    event_id TEXT PRIMARY KEY NOT NULL
        REFERENCES coordinator_watch_events(event_id) ON DELETE CASCADE,
    reason TEXT NOT NULL CHECK (reason = 'WakeFeatureRetired')
) STRICT;

INSERT INTO coordinator_event_feature_retirements (event_id, reason)
SELECT event_id, 'WakeFeatureRetired'
FROM coordinator_watch_events
WHERE source_occurrence_kind = 'wake';

CREATE TRIGGER retired_wake_event_reject_turn BEFORE INSERT ON durable_turns
WHEN EXISTS (
    SELECT 1 FROM coordinator_event_feature_retirements
    WHERE event_id = NEW.origin_subscription_event_id
)
BEGIN
    SELECT RAISE(ABORT, 'retired wake event has no delivery authority');
END;

CREATE TRIGGER retired_wake_event_reject_steering BEFORE INSERT ON steering_messages
WHEN EXISTS (
    SELECT 1 FROM coordinator_event_feature_retirements
    WHERE event_id = NEW.origin_subscription_event_id
)
BEGIN
    SELECT RAISE(ABORT, 'retired wake event has no delivery authority');
END;
