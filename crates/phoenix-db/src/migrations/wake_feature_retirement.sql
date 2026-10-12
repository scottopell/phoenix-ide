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
