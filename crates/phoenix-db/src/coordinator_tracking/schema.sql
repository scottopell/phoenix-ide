CREATE TABLE coordinator_tracking_registry (
 singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
 owner_product_id TEXT NOT NULL CHECK(length(trim(owner_product_id)) > 0),
 revision INTEGER NOT NULL DEFAULT 0 CHECK(typeof(revision) = 'integer' AND revision >= 0)
);
CREATE TABLE coordinator_tracking_items (
 id TEXT PRIMARY KEY NOT NULL,
 singleton INTEGER NOT NULL DEFAULT 1 REFERENCES coordinator_tracking_registry(singleton) CHECK(singleton = 1),
 kind TEXT NOT NULL CHECK(kind IN ('requested_work','bookmark')),
 title TEXT NOT NULL CHECK(length(trim(title)) BETWEEN 1 AND 160),
 note TEXT NOT NULL CHECK(length(CAST(note AS BLOB)) <= 8192),
 resumption TEXT NOT NULL CHECK(length(CAST(resumption AS BLOB)) <= 4096),
 current_retirement_revision INTEGER,
 revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision >= 1),
 reviewed_evidence_ordinal INTEGER NOT NULL CHECK(typeof(reviewed_evidence_ordinal)='integer' AND reviewed_evidence_ordinal >= -1),
 created_at_us INTEGER NOT NULL CHECK(typeof(created_at_us)='integer' AND created_at_us >= 0),
 updated_at_us INTEGER NOT NULL CHECK(typeof(updated_at_us)='integer' AND updated_at_us >= created_at_us),
 reviewed_at_us INTEGER CHECK(reviewed_at_us IS NULL OR (typeof(reviewed_at_us)='integer' AND reviewed_at_us >= 0)),
 FOREIGN KEY(id,current_retirement_revision) REFERENCES coordinator_tracking_retirements(item_id,item_revision) DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE coordinator_tracking_references (
 item_id TEXT NOT NULL REFERENCES coordinator_tracking_items(id),
 ordinal INTEGER NOT NULL CHECK(typeof(ordinal)='integer' AND ordinal >= 0),
 kind TEXT NOT NULL CHECK(kind IN ('conversation','message','pull_request')),
 instance_id TEXT,
 product_id TEXT,
 transcript_id TEXT,
 message_id TEXT,
 repository TEXT,
 pr_number INTEGER,
 PRIMARY KEY(item_id, ordinal),
 CHECK(instance_id IS NULL OR length(trim(instance_id)) > 0),
 CHECK(product_id IS NULL OR length(trim(product_id)) > 0),
 CHECK(transcript_id IS NULL OR length(trim(transcript_id)) > 0),
 CHECK(message_id IS NULL OR length(trim(message_id)) > 0),
 CHECK(repository IS NULL OR length(trim(repository)) > 0),
 CHECK((kind='conversation' AND instance_id IS NOT NULL AND product_id IS NOT NULL AND transcript_id IS NULL AND message_id IS NULL AND repository IS NULL AND pr_number IS NULL) OR
       (kind='message' AND instance_id IS NOT NULL AND product_id IS NULL AND transcript_id IS NOT NULL AND message_id IS NOT NULL AND repository IS NULL AND pr_number IS NULL) OR
       (kind='pull_request' AND instance_id IS NULL AND product_id IS NULL AND transcript_id IS NULL AND message_id IS NULL AND repository IS NOT NULL AND typeof(pr_number)='integer' AND pr_number > 0))
);
CREATE TABLE coordinator_tracking_evidence (
 item_id TEXT NOT NULL REFERENCES coordinator_tracking_items(id),
 ordinal INTEGER NOT NULL CHECK(typeof(ordinal)='integer' AND ordinal >= 0),
 instance_id TEXT NOT NULL CHECK(length(trim(instance_id)) > 0),
 transcript_id TEXT NOT NULL CHECK(length(trim(transcript_id)) > 0),
 message_id TEXT NOT NULL CHECK(length(trim(message_id)) > 0),
 quotation TEXT NOT NULL CHECK(length(CAST(quotation AS BLOB)) BETWEEN 1 AND 4096),
 digest TEXT NOT NULL,
 recorded_at_us INTEGER NOT NULL CHECK(typeof(recorded_at_us)='integer' AND recorded_at_us >= 0),
 PRIMARY KEY(item_id, ordinal),
 UNIQUE(item_id, instance_id, transcript_id, message_id, digest)
);
CREATE TABLE coordinator_tracking_operations (
 source_transcript_id TEXT NOT NULL,
 source_message_id TEXT NOT NULL,
 source_tool_use_id TEXT NOT NULL,
 fingerprint TEXT NOT NULL,
 operation_kind TEXT NOT NULL CHECK(operation_kind IN ('record','update','append_evidence','retire','reopen')),
 item_id TEXT NOT NULL REFERENCES coordinator_tracking_items(id),
 item_revision INTEGER NOT NULL,
 registry_revision INTEGER NOT NULL,
 PRIMARY KEY(source_transcript_id, source_message_id, source_tool_use_id)
);
CREATE UNIQUE INDEX coordinator_tracking_one_record_call ON coordinator_tracking_operations(item_id) WHERE operation_kind='record';
CREATE TABLE coordinator_tracking_retirements (
 item_id TEXT NOT NULL REFERENCES coordinator_tracking_items(id),
 item_revision INTEGER NOT NULL CHECK(typeof(item_revision)='integer' AND item_revision > 0),
 disposition TEXT NOT NULL CHECK(disposition IN ('resolved','dismissed')),
 reason TEXT NOT NULL CHECK(length(trim(reason)) BETWEEN 1 AND 4096),
 recorded_at_us INTEGER NOT NULL CHECK(typeof(recorded_at_us)='integer' AND recorded_at_us >= 0),
 PRIMARY KEY(item_id,item_revision)
);
CREATE TABLE coordinator_tracking_observations (
 transcript_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations(id),
 input_message_id TEXT NOT NULL,
 registry_revision INTEGER NOT NULL,
 message_id TEXT NOT NULL,
 cue_item_id TEXT REFERENCES coordinator_tracking_items(id),
 cue_at_us INTEGER CHECK(cue_at_us IS NULL OR (typeof(cue_at_us)='integer' AND cue_at_us >= 0))
);
CREATE TABLE coordinator_tracking_attention (
 item_id TEXT PRIMARY KEY NOT NULL REFERENCES coordinator_tracking_items(id),
 surfaced_at_us INTEGER NOT NULL CHECK(typeof(surfaced_at_us)='integer' AND surfaced_at_us >= 0),
 evidence_ordinal INTEGER NOT NULL CHECK(typeof(evidence_ordinal)='integer' AND evidence_ordinal >= 0)
);
