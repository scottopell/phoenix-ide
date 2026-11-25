ALTER TABLE coordinator_scan_evidence_origins RENAME TO coordinator_scan_evidence_origins_before_172;

CREATE TABLE coordinator_scan_evidence_origins (
 item_id TEXT NOT NULL,
 evidence_ordinal INTEGER NOT NULL,
 origin_kind TEXT NOT NULL CHECK(
   typeof(origin_kind)='text' AND
   -- Unicode White_Space; one-argument SQLite trim removes only U+0020.
   length(trim(origin_kind,char(9,10,11,12,13,32,133,160,5760,8192,8193,8194,8195,
                                8196,8197,8198,8199,8200,8201,8202,8232,8233,8239,8287,12288)))>0
 ),
 source_role TEXT NOT NULL CHECK(source_role IN ('user','assistant')),
 source_generation INTEGER NOT NULL CHECK(typeof(source_generation)='integer' AND source_generation>0),
 fragment_id TEXT NOT NULL CHECK(length(trim(fragment_id))>0),
 utf8_start INTEGER NOT NULL CHECK(typeof(utf8_start)='integer' AND utf8_start>=0),
 utf8_end INTEGER NOT NULL CHECK(typeof(utf8_end)='integer' AND utf8_end>utf8_start),
 attempt_id INTEGER NOT NULL REFERENCES coordinator_scan_attempts(id),
 PRIMARY KEY(item_id,evidence_ordinal),
 FOREIGN KEY(item_id,evidence_ordinal) REFERENCES coordinator_tracking_evidence(item_id,ordinal)
);

INSERT INTO coordinator_scan_evidence_origins (item_id,evidence_ordinal,origin_kind,source_role,source_generation,fragment_id,utf8_start,utf8_end,attempt_id)
SELECT item_id,evidence_ordinal,origin_kind,source_role,source_generation,fragment_id,utf8_start,utf8_end,attempt_id
FROM coordinator_scan_evidence_origins_before_172;

DROP TABLE coordinator_scan_evidence_origins_before_172;
