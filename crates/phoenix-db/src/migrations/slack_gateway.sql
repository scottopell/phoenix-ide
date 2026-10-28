-- REQ-SG-005/009/011/013. Existing inputs and origin columns are preserved.
PRAGMA writable_schema = ON;
UPDATE sqlite_schema SET sql = replace(sql, '''remote_coordinator''))', '''remote_coordinator'', ''slack_gateway''))')
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
 status TEXT NOT NULL DEFAULT 'initializing', last_scan_at_unix_micros INTEGER CHECK(last_scan_at_unix_micros IS NULL OR (typeof(last_scan_at_unix_micros)='integer' AND last_scan_at_unix_micros>=0)), last_error TEXT,
 calls_last_pass INTEGER NOT NULL DEFAULT 0, pages_last_pass INTEGER NOT NULL DEFAULT 0,
 retry_at_us INTEGER CHECK(retry_at_us IS NULL OR (typeof(retry_at_us)='integer' AND retry_at_us>=0)),
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
 accepted_at_unix_micros INTEGER NOT NULL CHECK(typeof(accepted_at_unix_micros)='integer' AND accepted_at_unix_micros>=0), eyes_confirmed INTEGER NOT NULL DEFAULT 0 CHECK(eyes_confirmed IN (0,1)),
 admission_state TEXT NOT NULL DEFAULT 'accepted' CHECK(admission_state IN ('accepted','admitted','rejected')),
 input_id TEXT NOT NULL UNIQUE, transcript_id TEXT, failure TEXT,
 observed_at_unix_micros INTEGER NOT NULL CHECK(typeof(observed_at_unix_micros)='integer' AND observed_at_unix_micros>=0),
 trigger_kind TEXT NOT NULL CHECK(trigger_kind IN ('prefix','reaction')),
 eyes_failure TEXT CHECK(eyes_failure IS NULL OR (eyes_confirmed=0 AND length(trim(eyes_failure))>0)),
 UNIQUE(workspace,owner,channel,message_ts),
 CHECK((kind = 'baseline') = (binding_id IS NULL))
);
CREATE TABLE slack_gateway_returns (
 return_id TEXT PRIMARY KEY, binding_id TEXT NOT NULL REFERENCES slack_gateway_bindings(binding_id),
 occurrence_key TEXT NOT NULL, destination TEXT NOT NULL CHECK(destination IN ('thread','private')),
 text TEXT NOT NULL, native_message_id TEXT,
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','attempting','submitted','confirmed','known_unsent','uncertain','failed')),
 attempt_id TEXT, delivery_proof TEXT, notification_receipt TEXT, failure TEXT,
 notification_command TEXT CHECK(notification_command IS NULL OR (json_valid(notification_command) AND json_type(notification_command)='array' AND json_array_length(notification_command) BETWEEN 1 AND 16)),
 notification_url TEXT CHECK(notification_url IS NULL OR length(trim(notification_url))>0),
 thread_attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(typeof(thread_attempt_count)='integer' AND thread_attempt_count BETWEEN 0 AND 3),
 created_at_unix_micros INTEGER NOT NULL CHECK(typeof(created_at_unix_micros)='integer' AND created_at_unix_micros>=0), UNIQUE(binding_id,occurrence_key,destination),
 CHECK((state='confirmed') = (delivery_proof IS NOT NULL)),
 CHECK(state<>'submitted' OR (destination='private' AND notification_receipt IS NOT NULL AND length(trim(notification_receipt))>0 AND notification_command IS NOT NULL)),
 CHECK(destination='thread' OR state NOT IN ('known_unsent','confirmed'))
);
CREATE TRIGGER slack_gateway_notification_operation_immutable BEFORE UPDATE OF notification_command,notification_url ON slack_gateway_returns
WHEN (OLD.notification_command IS NOT NULL AND NEW.notification_command IS NOT OLD.notification_command) OR (OLD.notification_url IS NOT NULL AND NEW.notification_url IS NOT OLD.notification_url)
BEGIN SELECT RAISE(ABORT,'notification operation is immutable'); END;
CREATE TABLE slack_gateway_return_coverage (
 return_id TEXT NOT NULL REFERENCES slack_gateway_returns(return_id),
 request_id TEXT NOT NULL REFERENCES slack_gateway_requests(request_id), PRIMARY KEY(return_id,request_id)
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
 transcript_id TEXT NOT NULL,
 processed INTEGER NOT NULL DEFAULT 0 CHECK(processed IN (0,1)),
 context_exhausted INTEGER NOT NULL DEFAULT 0 CHECK(context_exhausted IN (0,1)),
 PRIMARY KEY(request_id,occurrence_key)
);
CREATE UNIQUE INDEX slack_gateway_explicit_thread ON slack_gateway_bindings(workspace,owner,channel,thread_ts) WHERE kind='explicit';
CREATE INDEX slack_gateway_binding_product ON slack_gateway_bindings(product_id);
CREATE INDEX slack_gateway_return_dispatch ON slack_gateway_returns(state,created_at_unix_micros);
CREATE INDEX slack_gateway_request_account ON slack_gateway_requests(workspace,owner,accepted_at_unix_micros);
CREATE TABLE slack_gateway_observations (
 workspace TEXT NOT NULL, owner TEXT NOT NULL, channel TEXT NOT NULL, message_ts TEXT NOT NULL,
 first_observed_at_unix_micros INTEGER NOT NULL CHECK(typeof(first_observed_at_unix_micros)='integer' AND first_observed_at_unix_micros>=0),
 PRIMARY KEY(workspace,owner,channel,message_ts)
);
