INSERT INTO app_settings(key,value) VALUES ('slack_gateway_saved_enabled','false');

CREATE TABLE slack_gateway_saved_inboxes (
 workspace TEXT NOT NULL, owner TEXT NOT NULL,
 baseline_complete INTEGER NOT NULL DEFAULT 0 CHECK(baseline_complete IN (0,1)),
 cursor TEXT, scan_in_progress INTEGER NOT NULL DEFAULT 0 CHECK(scan_in_progress IN (0,1)),
 last_scan_at_unix_micros INTEGER CHECK(last_scan_at_unix_micros IS NULL OR (typeof(last_scan_at_unix_micros)='integer' AND last_scan_at_unix_micros>=0)),
 last_error TEXT,
 PRIMARY KEY(workspace,owner),
 FOREIGN KEY(workspace,owner) REFERENCES slack_gateway_accounts(workspace,owner)
);

CREATE TABLE slack_gateway_requests_new (
 request_id TEXT PRIMARY KEY, workspace TEXT NOT NULL, owner TEXT NOT NULL, channel TEXT NOT NULL, message_ts TEXT NOT NULL,
 thread_ts TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('baseline','briefing','explicit')),
 instruction TEXT NOT NULL, context TEXT NOT NULL, source_url TEXT NOT NULL, binding_id TEXT REFERENCES slack_gateway_bindings(binding_id),
 accepted_at_unix_micros INTEGER NOT NULL CHECK(typeof(accepted_at_unix_micros)='integer' AND accepted_at_unix_micros>=0), eyes_confirmed INTEGER NOT NULL DEFAULT 0 CHECK(eyes_confirmed IN (0,1)),
 admission_state TEXT NOT NULL DEFAULT 'accepted' CHECK(admission_state IN ('accepted','admitted','rejected')),
 input_id TEXT NOT NULL UNIQUE, transcript_id TEXT, failure TEXT,
 observed_at_unix_micros INTEGER NOT NULL CHECK(typeof(observed_at_unix_micros)='integer' AND observed_at_unix_micros>=0),
 trigger_kind TEXT NOT NULL CHECK(trigger_kind IN ('prefix','reaction','saved')),
 capture_scope TEXT GENERATED ALWAYS AS (CASE WHEN trigger_kind='saved' THEN 'saved' ELSE 'emoji' END) VIRTUAL,
 eyes_failure TEXT CHECK(eyes_failure IS NULL OR (eyes_confirmed=0 AND length(trim(eyes_failure))>0)),
 UNIQUE(workspace,owner,channel,message_ts,capture_scope),
 CHECK((kind = 'baseline') = (binding_id IS NULL)),
 CHECK(trigger_kind<>'saved' OR (kind IN ('baseline','briefing') AND instruction='' AND eyes_confirmed=0 AND eyes_failure IS NULL))
);
INSERT INTO slack_gateway_requests_new (
 request_id,workspace,owner,channel,message_ts,thread_ts,kind,instruction,context,source_url,binding_id,
 accepted_at_unix_micros,eyes_confirmed,admission_state,input_id,transcript_id,failure,observed_at_unix_micros,trigger_kind,eyes_failure
)
SELECT request_id,workspace,owner,channel,message_ts,thread_ts,kind,instruction,context,source_url,binding_id,
 accepted_at_unix_micros,eyes_confirmed,admission_state,input_id,transcript_id,failure,observed_at_unix_micros,trigger_kind,eyes_failure
FROM slack_gateway_requests;
DROP TABLE slack_gateway_requests;
ALTER TABLE slack_gateway_requests_new RENAME TO slack_gateway_requests;
CREATE INDEX slack_gateway_request_account ON slack_gateway_requests(workspace,owner,accepted_at_unix_micros);

CREATE TABLE slack_gateway_observations_new (
 workspace TEXT NOT NULL, owner TEXT NOT NULL, channel TEXT NOT NULL, message_ts TEXT NOT NULL,
 capture_scope TEXT NOT NULL CHECK(capture_scope IN ('emoji','saved')),
 first_observed_at_unix_micros INTEGER NOT NULL CHECK(typeof(first_observed_at_unix_micros)='integer' AND first_observed_at_unix_micros>=0),
 PRIMARY KEY(workspace,owner,channel,message_ts,capture_scope)
);
INSERT INTO slack_gateway_observations_new
SELECT workspace,owner,channel,message_ts,'emoji',first_observed_at_unix_micros FROM slack_gateway_observations;
DROP TABLE slack_gateway_observations;
ALTER TABLE slack_gateway_observations_new RENAME TO slack_gateway_observations;
