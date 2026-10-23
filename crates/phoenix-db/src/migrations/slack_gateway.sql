-- REQ-SG-005/009/011/013. Existing inputs and origin columns are preserved.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema SET sql = replace(sql, '''subscription_event''))', '''subscription_event'', ''slack_gateway''))')
WHERE type = 'table' AND name IN ('messages','steering_messages','durable_turns');
PRAGMA writable_schema = RESET;
ALTER TABLE messages ADD COLUMN origin_slack_request_id TEXT REFERENCES slack_gateway_requests(request_id);
CREATE TRIGGER messages_slack_origin_insert BEFORE INSERT ON messages
WHEN (NEW.origin_kind = 'slack_gateway') != (NEW.origin_slack_request_id IS NOT NULL AND length(trim(NEW.origin_slack_request_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid Slack origin'); END;
CREATE TRIGGER messages_slack_origin_update BEFORE UPDATE OF origin_kind, origin_slack_request_id ON messages
WHEN (NEW.origin_kind = 'slack_gateway') != (NEW.origin_slack_request_id IS NOT NULL AND length(trim(NEW.origin_slack_request_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid Slack origin'); END;
ALTER TABLE steering_messages ADD COLUMN origin_slack_request_id TEXT REFERENCES slack_gateway_requests(request_id);
CREATE TRIGGER steering_messages_slack_origin_insert BEFORE INSERT ON steering_messages
WHEN (NEW.origin_kind = 'slack_gateway') != (NEW.origin_slack_request_id IS NOT NULL AND length(trim(NEW.origin_slack_request_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid Slack origin'); END;
CREATE TRIGGER steering_messages_slack_origin_update BEFORE UPDATE OF origin_kind, origin_slack_request_id ON steering_messages
WHEN (NEW.origin_kind = 'slack_gateway') != (NEW.origin_slack_request_id IS NOT NULL AND length(trim(NEW.origin_slack_request_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid Slack origin'); END;
ALTER TABLE durable_turns ADD COLUMN origin_slack_request_id TEXT REFERENCES slack_gateway_requests(request_id);
CREATE TRIGGER durable_turns_slack_origin_insert BEFORE INSERT ON durable_turns
WHEN (NEW.origin_kind = 'slack_gateway') != (NEW.origin_slack_request_id IS NOT NULL AND length(trim(NEW.origin_slack_request_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid Slack origin'); END;
CREATE TRIGGER durable_turns_slack_origin_update BEFORE UPDATE OF origin_kind, origin_slack_request_id ON durable_turns
WHEN (NEW.origin_kind = 'slack_gateway') != (NEW.origin_slack_request_id IS NOT NULL AND length(trim(NEW.origin_slack_request_id)) > 0)
BEGIN SELECT RAISE(ABORT, 'invalid Slack origin'); END;
CREATE TABLE slack_gateway_accounts (
 workspace TEXT NOT NULL, owner TEXT NOT NULL, baseline_complete INTEGER NOT NULL DEFAULT 0 CHECK(baseline_complete IN (0,1)),
 status TEXT NOT NULL DEFAULT 'initializing', last_scan_at INTEGER, last_error TEXT,
 PRIMARY KEY(workspace,owner)
);
CREATE TABLE slack_gateway_bindings (
 binding_id TEXT PRIMARY KEY, workspace TEXT NOT NULL, owner TEXT NOT NULL, channel TEXT NOT NULL, thread_ts TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('briefing','explicit')), creation_id TEXT NOT NULL UNIQUE,
 product_id TEXT, failure TEXT
);
CREATE TABLE slack_gateway_requests (
 request_id TEXT PRIMARY KEY, workspace TEXT NOT NULL, owner TEXT NOT NULL, channel TEXT NOT NULL, message_ts TEXT NOT NULL,
 thread_ts TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('baseline','briefing','explicit')),
 instruction TEXT NOT NULL, context TEXT NOT NULL, source_url TEXT NOT NULL, binding_id TEXT REFERENCES slack_gateway_bindings(binding_id),
 accepted_at INTEGER NOT NULL, eyes_confirmed INTEGER NOT NULL DEFAULT 0 CHECK(eyes_confirmed IN (0,1)),
 admission_state TEXT NOT NULL DEFAULT 'accepted' CHECK(admission_state IN ('accepted','admitted','rejected')),
 input_id TEXT NOT NULL UNIQUE, transcript_id TEXT, failure TEXT,
 UNIQUE(workspace,owner,channel,message_ts),
 CHECK((kind = 'baseline') = (binding_id IS NULL))
);
CREATE TABLE slack_gateway_returns (
 return_id TEXT PRIMARY KEY, binding_id TEXT NOT NULL REFERENCES slack_gateway_bindings(binding_id),
 occurrence_key TEXT NOT NULL, destination TEXT NOT NULL CHECK(destination IN ('thread','private')),
 text TEXT NOT NULL, native_message_id TEXT,
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','attempting','submitted','confirmed','known_unsent','uncertain','failed')),
 attempt_id TEXT, delivery_proof TEXT, notification_receipt TEXT, failure TEXT,
 notification_command TEXT, created_at INTEGER NOT NULL, UNIQUE(binding_id,occurrence_key,destination),
 CHECK((state='confirmed') = (delivery_proof IS NOT NULL))
);
CREATE TABLE slack_gateway_return_coverage (
 return_id TEXT NOT NULL REFERENCES slack_gateway_returns(return_id),
 request_id TEXT NOT NULL REFERENCES slack_gateway_requests(request_id), PRIMARY KEY(return_id,request_id)
);
CREATE TABLE slack_gateway_questions (
 return_id TEXT NOT NULL REFERENCES slack_gateway_returns(return_id), question_id TEXT NOT NULL,
 transcript_id TEXT NOT NULL, answered_by TEXT REFERENCES slack_gateway_requests(request_id), PRIMARY KEY(return_id,question_id)
);

PRAGMA writable_schema = ON;
UPDATE sqlite_schema SET sql = replace(sql, 'objective_origin IN (''unknown_historical'', ''user_api'')', 'objective_origin IN (''unknown_historical'', ''user_api'', ''slack_gateway'')') WHERE type='table' AND name='product_creation_jobs';
PRAGMA writable_schema = RESET;
ALTER TABLE product_creation_jobs ADD COLUMN objective_slack_request_id TEXT REFERENCES slack_gateway_requests(request_id);
CREATE TRIGGER creation_slack_origin_insert BEFORE INSERT ON product_creation_jobs
WHEN (NEW.objective_origin='slack_gateway') != (NEW.objective_slack_request_id IS NOT NULL)
BEGIN SELECT RAISE(ABORT,'invalid creation Slack origin'); END;
CREATE TABLE slack_gateway_discovery (
 workspace TEXT NOT NULL, owner TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('prefix','reaction')),
 after_date TEXT NOT NULL, cursor TEXT, complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0,1)),
 PRIMARY KEY(workspace,owner,kind)
);
CREATE TABLE slack_gateway_native_outcomes (
 request_id TEXT NOT NULL REFERENCES slack_gateway_requests(request_id), occurrence_key TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('Completed','Cancelled','Failed','question')),
 transcript_id TEXT NOT NULL, question_id TEXT, question_body TEXT,
 processed INTEGER NOT NULL DEFAULT 0 CHECK(processed IN (0,1)),
 PRIMARY KEY(request_id,occurrence_key)
);
ALTER TABLE slack_gateway_returns ADD COLUMN response_kind TEXT NOT NULL DEFAULT 'answer' CHECK(response_kind IN ('answer','clarification','failure','notice'));
ALTER TABLE slack_gateway_requests ADD COLUMN question_id TEXT;
ALTER TABLE slack_gateway_requests ADD COLUMN question_transcript TEXT;

CREATE UNIQUE INDEX slack_gateway_explicit_thread ON slack_gateway_bindings(workspace,owner,channel,thread_ts) WHERE kind='explicit';
CREATE INDEX slack_gateway_binding_product ON slack_gateway_bindings(product_id);
CREATE INDEX slack_gateway_return_dispatch ON slack_gateway_returns(state,created_at);
CREATE INDEX slack_gateway_request_account ON slack_gateway_requests(workspace,owner,accepted_at);
ALTER TABLE slack_gateway_accounts ADD COLUMN calls_last_pass INTEGER NOT NULL DEFAULT 0;
ALTER TABLE slack_gateway_accounts ADD COLUMN pages_last_pass INTEGER NOT NULL DEFAULT 0;
ALTER TABLE slack_gateway_accounts ADD COLUMN retry_at_us INTEGER;
ALTER TABLE slack_gateway_requests ADD COLUMN observed_at INTEGER NOT NULL DEFAULT 0 CHECK(observed_at>=0);

-- Native context exhaustion may transfer the same request to a successor.
ALTER TABLE slack_gateway_native_outcomes ADD COLUMN context_exhausted INTEGER NOT NULL DEFAULT 0 CHECK(context_exhausted IN (0,1));
ALTER TABLE slack_gateway_questions ADD COLUMN retired_reason TEXT;
