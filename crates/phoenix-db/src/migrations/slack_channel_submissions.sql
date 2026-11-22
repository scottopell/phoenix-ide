INSERT INTO app_settings(key,value) VALUES ('slack_gateway_inbox_channel','');

CREATE TABLE slack_gateway_channel_inboxes (
 workspace TEXT NOT NULL, owner TEXT NOT NULL, channel TEXT NOT NULL CHECK(length(channel)>0),
 baseline_complete INTEGER NOT NULL DEFAULT 0 CHECK(baseline_complete IN (0,1)),
 cursor TEXT, after_date TEXT, scan_in_progress INTEGER NOT NULL DEFAULT 0 CHECK(scan_in_progress IN (0,1)),
 last_scan_at_unix_micros INTEGER, last_error TEXT,
 PRIMARY KEY(workspace,owner,channel),
 FOREIGN KEY(workspace,owner) REFERENCES slack_gateway_accounts(workspace,owner)
);

PRAGMA writable_schema = ON;
UPDATE sqlite_schema SET sql = replace(sql,
 'CHECK(trigger_kind IN (''prefix'',''reaction'',''saved''))',
 'CHECK(trigger_kind IN (''prefix'',''reaction'',''saved'',''inbox'')) CHECK(trigger_kind<>''inbox'' OR (kind IN (''baseline'',''explicit'') AND message_ts=thread_ts))')
 WHERE type='table' AND name='slack_gateway_requests';
PRAGMA writable_schema = RESET;
