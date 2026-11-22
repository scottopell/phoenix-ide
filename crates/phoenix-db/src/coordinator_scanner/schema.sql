CREATE TABLE coordinator_scan_state (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1) REFERENCES coordinator_tracking_registry(singleton),
 activation_member_ordinal INTEGER NOT NULL CHECK(typeof(activation_member_ordinal)='integer' AND activation_member_ordinal>=0),
 discovery_next_ordinal INTEGER NOT NULL CHECK(typeof(discovery_next_ordinal)='integer' AND discovery_next_ordinal>=0),
 live_streak INTEGER NOT NULL DEFAULT 0 CHECK(live_streak BETWEEN 0 AND 3),
 last_blocker TEXT CHECK(last_blocker IS NULL OR last_blocker IN ('route_unavailable','global_budget_paused','historical_budget_paused','retry_backoff','invalid_output','source_overflow','source_unavailable','inference_failed')),
 next_resume_at_us INTEGER CHECK(next_resume_at_us IS NULL OR (typeof(next_resume_at_us)='integer' AND next_resume_at_us>=0)),
 observed_at_us INTEGER NOT NULL CHECK(typeof(observed_at_us)='integer' AND observed_at_us>=0)
);
CREATE TABLE coordinator_scan_members (
 transcript_id TEXT PRIMARY KEY NOT NULL CHECK(length(trim(transcript_id))>0),
 singleton INTEGER NOT NULL DEFAULT 1 REFERENCES coordinator_scan_state(singleton) CHECK(singleton=1),
 continuation_ordinal INTEGER NOT NULL UNIQUE CHECK(typeof(continuation_ordinal)='integer' AND continuation_ordinal>=0),
 generation INTEGER NOT NULL CHECK(typeof(generation)='integer' AND generation>0),
 activation_boundary INTEGER NOT NULL CHECK(typeof(activation_boundary)='integer' AND activation_boundary>=0),
 frontier_revision INTEGER NOT NULL CHECK(typeof(frontier_revision)='integer' AND frontier_revision>=0),
 live_seq INTEGER NOT NULL CHECK(typeof(live_seq)='integer' AND live_seq>=0),
 live_message_id TEXT, live_part INTEGER, live_offset INTEGER, live_digest TEXT,
 historical_seq INTEGER NOT NULL CHECK(typeof(historical_seq)='integer' AND historical_seq>=0),
 historical_message_id TEXT, historical_part INTEGER, historical_offset INTEGER, historical_digest TEXT,
 selected_at_us INTEGER NOT NULL DEFAULT 0 CHECK(typeof(selected_at_us)='integer' AND selected_at_us>=0),
 CHECK((live_message_id IS NULL AND live_part IS NULL AND live_offset IS NULL AND live_digest IS NULL) OR
       (live_message_id IS NOT NULL AND live_part IS NOT NULL AND live_offset IS NOT NULL AND live_digest IS NOT NULL AND length(trim(live_message_id))>0 AND typeof(live_part)='integer' AND live_part>=0 AND typeof(live_offset)='integer' AND live_offset>=0 AND length(live_digest)=64 AND live_digest NOT GLOB '*[^0-9a-f]*')),
 CHECK((historical_message_id IS NULL AND historical_part IS NULL AND historical_offset IS NULL AND historical_digest IS NULL) OR
       (historical_message_id IS NOT NULL AND historical_part IS NOT NULL AND historical_offset IS NOT NULL AND historical_digest IS NOT NULL AND length(trim(historical_message_id))>0 AND typeof(historical_part)='integer' AND historical_part>=0 AND typeof(historical_offset)='integer' AND historical_offset>=0 AND length(historical_digest)=64 AND historical_digest NOT GLOB '*[^0-9a-f]*'))
);
CREATE TABLE coordinator_scan_work (
 binding_id TEXT PRIMARY KEY NOT NULL CHECK(length(binding_id)=64 AND binding_id NOT GLOB '*[^0-9a-f]*'),
 transcript_id TEXT NOT NULL REFERENCES coordinator_scan_members(transcript_id),
 source_generation INTEGER NOT NULL CHECK(typeof(source_generation)='integer' AND source_generation>0),
 lane TEXT NOT NULL CHECK(lane IN ('live','historical')),
 frontier_revision INTEGER NOT NULL CHECK(typeof(frontier_revision)='integer' AND frontier_revision>=0),
 registry_revision INTEGER NOT NULL CHECK(typeof(registry_revision)='integer' AND registry_revision>=0),
 source_digest TEXT NOT NULL CHECK(length(source_digest)=64 AND source_digest NOT GLOB '*[^0-9a-f]*'),
 connection TEXT NOT NULL CHECK(connection IN ('codex','openai_responses')),
 endpoint_hash TEXT NOT NULL CHECK(length(endpoint_hash)=64 AND endpoint_hash NOT GLOB '*[^0-9a-f]*'),
 prompt_hash TEXT NOT NULL CHECK(length(prompt_hash)=64 AND prompt_hash NOT GLOB '*[^0-9a-f]*'),
 cycle INTEGER NOT NULL DEFAULT 0 CHECK(typeof(cycle)='integer' AND cycle>=0),
 resumes_at_us INTEGER CHECK(resumes_at_us IS NULL OR (typeof(resumes_at_us)='integer' AND resumes_at_us>=0))
);
CREATE TABLE coordinator_scan_attempts (
 id INTEGER PRIMARY KEY,
 binding_id TEXT NOT NULL REFERENCES coordinator_scan_work(binding_id),
 cycle INTEGER NOT NULL CHECK(typeof(cycle)='integer' AND cycle>=0),
 ordinal INTEGER NOT NULL CHECK(ordinal IN (1,2)),
 reserved_at_us INTEGER NOT NULL CHECK(typeof(reserved_at_us)='integer' AND reserved_at_us>=0),
 input_bytes INTEGER NOT NULL CHECK(typeof(input_bytes)='integer' AND input_bytes BETWEEN 1 AND 24576),
 state TEXT NOT NULL CHECK(state IN ('reserved','applied','conflict','rejected','failed')),
 failure_kind TEXT CHECK(failure_kind IS NULL OR failure_kind IN ('provider','timeout','invalid_output','storage')),
 failure_detail TEXT CHECK(failure_detail IS NULL OR length(CAST(failure_detail AS BLOB))<=1024),
 elapsed_ms INTEGER CHECK(elapsed_ms IS NULL OR (typeof(elapsed_ms)='integer' AND elapsed_ms>=0)),
 input_tokens INTEGER, cache_read_tokens INTEGER, cache_creation_tokens INTEGER, output_tokens INTEGER,
 UNIQUE(binding_id,cycle,ordinal),
 CHECK(input_tokens IS NULL OR (typeof(input_tokens)='integer' AND input_tokens>=0)),
 CHECK(cache_read_tokens IS NULL OR (typeof(cache_read_tokens)='integer' AND cache_read_tokens>=0)),
 CHECK(cache_creation_tokens IS NULL OR (typeof(cache_creation_tokens)='integer' AND cache_creation_tokens>=0)),
 CHECK(output_tokens IS NULL OR (typeof(output_tokens)='integer' AND output_tokens>=0)),
 CHECK((state='reserved' AND elapsed_ms IS NULL AND failure_kind IS NULL AND failure_detail IS NULL) OR
       (state IN ('applied','conflict') AND elapsed_ms IS NOT NULL AND failure_kind IS NULL AND failure_detail IS NULL) OR
       (state='rejected' AND elapsed_ms IS NOT NULL AND failure_kind='invalid_output' AND failure_detail IS NOT NULL) OR
       (state='failed' AND elapsed_ms IS NOT NULL AND failure_kind IS NOT NULL AND failure_detail IS NOT NULL))
);
CREATE INDEX coordinator_scan_attempt_budget ON coordinator_scan_attempts(reserved_at_us,binding_id);
CREATE TABLE coordinator_scan_applications (
 binding_id TEXT PRIMARY KEY NOT NULL REFERENCES coordinator_scan_work(binding_id),
 attempt_id INTEGER NOT NULL REFERENCES coordinator_scan_attempts(id),
 result_digest TEXT NOT NULL CHECK(length(result_digest)=64 AND result_digest NOT GLOB '*[^0-9a-f]*'),
 registry_revision INTEGER NOT NULL CHECK(typeof(registry_revision)='integer' AND registry_revision>=0),
 applied_at_us INTEGER NOT NULL CHECK(typeof(applied_at_us)='integer' AND applied_at_us>=0)
);
CREATE TABLE coordinator_scan_item_origins (
 item_id TEXT PRIMARY KEY NOT NULL REFERENCES coordinator_tracking_items(id),
 attempt_id INTEGER NOT NULL REFERENCES coordinator_scan_attempts(id)
);
CREATE TABLE coordinator_scan_evidence_origins (
 item_id TEXT NOT NULL,
 evidence_ordinal INTEGER NOT NULL,
 origin_kind TEXT NOT NULL CHECK(origin_kind IN ('unknown_historical','user_api','internal_conversation','system_generated','slack_gateway','remote_api','remote_conversation','remote_coordinator','subscription_event','scheduled_reminder')),
 source_role TEXT NOT NULL CHECK(source_role IN ('user','assistant')),
 source_generation INTEGER NOT NULL CHECK(typeof(source_generation)='integer' AND source_generation>0),
 fragment_id TEXT NOT NULL CHECK(length(trim(fragment_id))>0),
 utf8_start INTEGER NOT NULL CHECK(typeof(utf8_start)='integer' AND utf8_start>=0),
 utf8_end INTEGER NOT NULL CHECK(typeof(utf8_end)='integer' AND utf8_end>utf8_start),
 attempt_id INTEGER NOT NULL REFERENCES coordinator_scan_attempts(id),
 PRIMARY KEY(item_id,evidence_ordinal),
 FOREIGN KEY(item_id,evidence_ordinal) REFERENCES coordinator_tracking_evidence(item_id,ordinal)
);
