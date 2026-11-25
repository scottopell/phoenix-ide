//! Durable personal-channel scan progress (REQ-SG-020).
use super::{Database, DbError, DbResult, SlackScanProgress};
use sqlx::Row;

impl Database {
    pub async fn slack_channel_inbox(
        &self,
        workspace: &str,
        owner: &str,
        channel: &str,
    ) -> DbResult<Option<SlackScanProgress>> {
        Ok(sqlx::query("SELECT baseline_complete,cursor,scan_in_progress,last_scan_at_unix_micros,last_error FROM slack_gateway_channel_inboxes WHERE workspace=?1 AND owner=?2 AND channel=?3")
            .bind(workspace).bind(owner).bind(channel).fetch_optional(self.pool()).await?
            .map(|r| SlackScanProgress { baseline_complete:r.get("baseline_complete"),cursor:r.get("cursor"),scan_in_progress:r.get("scan_in_progress"),last_scan_at_unix_micros:r.get("last_scan_at_unix_micros"),last_error:r.get("last_error") }))
    }

    pub async fn slack_channel_begin_scan(
        &self,
        workspace: &str,
        owner: &str,
        channel: &str,
        after: &str,
    ) -> DbResult<bool> {
        self.slack_baseline_complete(workspace, owner).await?;
        sqlx::query("INSERT INTO slack_gateway_channel_inboxes(workspace,owner,channel,scan_in_progress,after_date) VALUES (?1,?2,?3,1,?4) ON CONFLICT(workspace,owner,channel) DO UPDATE SET after_date=CASE WHEN scan_in_progress=0 THEN excluded.after_date ELSE after_date END,scan_in_progress=1")
            .bind(workspace).bind(owner).bind(channel).bind(after).execute(self.pool()).await?;
        Ok(self
            .slack_channel_inbox(workspace, owner, channel)
            .await?
            .ok_or_else(|| DbError::Serialization("Channel inbox unavailable".into()))?
            .baseline_complete)
    }

    pub async fn slack_channel_checkpoint(
        &self,
        workspace: &str,
        owner: &str,
        channel: &str,
        cursor: Option<&str>,
    ) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_channel_inboxes SET cursor=?4,scan_in_progress=?5,baseline_complete=MAX(baseline_complete,?6),last_scan_at_unix_micros=CASE WHEN ?6 THEN ?7 ELSE last_scan_at_unix_micros END,last_error=CASE WHEN ?6 THEN NULL ELSE last_error END WHERE workspace=?1 AND owner=?2 AND channel=?3")
            .bind(workspace).bind(owner).bind(channel).bind(cursor).bind(cursor.is_some()).bind(cursor.is_none()).bind(chrono::Utc::now().timestamp_micros()).execute(self.pool()).await?;
        Ok(())
    }

    pub async fn slack_channel_failure(
        &self,
        workspace: &str,
        owner: &str,
        channel: &str,
        reason: &str,
        restart: bool,
    ) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_channel_inboxes SET last_error=?4,cursor=CASE WHEN ?5 THEN NULL ELSE cursor END WHERE workspace=?1 AND owner=?2 AND channel=?3")
            .bind(workspace).bind(owner).bind(channel).bind(reason).bind(restart).execute(self.pool()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn channel_upgrade_preserves_accepted_identity_and_return_coverage() {
        // REQ-SG-020/005
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("upgrade.db");
        let db = Database::open_prefix_for_test(path.to_str().unwrap(), 166)
            .await
            .unwrap();
        sqlx::raw_sql("INSERT INTO slack_gateway_accounts(workspace,owner) VALUES ('example.slack.com','UOWNER');
          INSERT INTO slack_gateway_bindings(binding_id,workspace,owner,channel,thread_ts,kind,creation_id) VALUES ('binding','example.slack.com','UOWNER','CINBOX','100.000001','explicit','creation');
          INSERT INTO slack_gateway_requests(request_id,workspace,owner,channel,message_ts,thread_ts,kind,instruction,context,source_url,binding_id,accepted_at_unix_micros,input_id,observed_at_unix_micros,trigger_kind) VALUES ('request','example.slack.com','UOWNER','CINBOX','100.000001','100.000001','explicit','investigate','quoted','source','binding',1,'input',1,'prefix');
          INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,created_at_unix_micros) VALUES ('return','binding','outcome','thread','answer',1);
          INSERT INTO slack_gateway_return_coverage(return_id,request_id) VALUES ('return','request');")
            .execute(db.pool()).await.unwrap();
        assert_eq!(
            crate::migrations::run_through_for_test(db.pool(), 167)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            db.slack_requests("example.slack.com", "UOWNER")
                .await
                .unwrap()[0]
                .request_id,
            "request"
        );
        assert_eq!(
            db.slack_returns("example.slack.com", "UOWNER")
                .await
                .unwrap()[0]
                .request_ids,
            vec!["request"]
        );
        assert_eq!(db.slack_configuration().await.unwrap(), None);
        sqlx::query(
            "UPDATE slack_gateway_requests SET trigger_kind='inbox' WHERE request_id='request'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT capture_scope FROM slack_gateway_requests")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            "emoji"
        );
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        drop(db);
        let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(
            crate::migrations::run_through_for_test(reopened.pool(), 167)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            reopened
                .slack_requests("example.slack.com", "UOWNER")
                .await
                .unwrap()[0]
                .source
                .trigger_kind,
            super::super::SlackTriggerKind::Inbox
        );
    }
}
