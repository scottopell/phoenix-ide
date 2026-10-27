//! Normalized Slack acceptance and return evidence (REQ-SG-005/007/011/013).
#![allow(clippy::missing_errors_doc)]
use super::{Database, DbError, DbResult};
use crate::product_conversation_read::ORDINARY_PRODUCT_ROUTE_PREDICATE;
use serde::{Deserialize, Serialize};
use sqlx::Row;

const EYES_OUTCOME_UNKNOWN: &str =
    "Acknowledgment outcome unknown; the one-time attempt is not retried";
const CLARIFICATION_HANDOFF: &str = "Phoenix needs your input. Continue in Phoenix:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SlackRequestKind {
    Briefing,
    Explicit,
}
impl SlackRequestKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Briefing => "briefing",
            Self::Explicit => "explicit",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SlackTriggerKind {
    Prefix,
    Reaction,
    UnknownHistorical,
}
impl SlackTriggerKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Reaction => "reaction",
            Self::UnknownHistorical => "unknown_historical",
        }
    }
    fn decode(value: &str) -> Result<Self, sqlx::Error> {
        match value {
            "prefix" => Ok(Self::Prefix),
            "reaction" => Ok(Self::Reaction),
            "unknown_historical" => Ok(Self::UnknownHistorical),
            _ => Err(sqlx::Error::Decode("Invalid Slack trigger kind".into())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackSource {
    pub workspace: String,
    pub owner: String,
    pub channel: String,
    pub message_ts: String,
    pub thread_ts: String,
    pub source_url: String,
    pub instruction: String,
    pub context: String,
    pub observed_at_unix_micros: i64,
    pub kind: SlackRequestKind,
    pub trigger_kind: SlackTriggerKind,
}
/// Evidence needed before a discovery page can advance past a verified source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackSourceAcceptance {
    Recorded,
    Inactive,
}
#[derive(Debug, Clone, Serialize)]
pub struct SlackRequest {
    pub request_id: String,
    pub source: SlackSource,
    pub binding_id: String,
    pub creation_id: String,
    pub product_id: Option<String>,
    pub conversation_available: bool,
    pub input_id: String,
    pub transcript_id: Option<String>,
    pub admitted: bool,
    pub accepted_at_us: i64,
    pub eyes_confirmed: bool,
    pub eyes_failure: Option<String>,
    pub failure: Option<String>,
    pub question_id: Option<String>,
    pub question_transcript: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackReturnState {
    Pending,
    Attempting,
    Submitted,
    Confirmed,
    KnownUnsent,
    Uncertain,
    Failed,
}
impl SlackReturnState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Attempting => "attempting",
            Self::Submitted => "submitted",
            Self::Confirmed => "confirmed",
            Self::KnownUnsent => "known_unsent",
            Self::Uncertain => "uncertain",
            Self::Failed => "failed",
        }
    }
    fn decode(value: &str) -> Result<Self, sqlx::Error> {
        match value {
            "pending" => Ok(Self::Pending),
            "attempting" => Ok(Self::Attempting),
            "submitted" => Ok(Self::Submitted),
            "confirmed" => Ok(Self::Confirmed),
            "known_unsent" => Ok(Self::KnownUnsent),
            "uncertain" => Ok(Self::Uncertain),
            "failed" => Ok(Self::Failed),
            _ => Err(sqlx::Error::Decode("Invalid Slack return state".into())),
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct SlackReturn {
    pub return_id: String,
    #[serde(skip_serializing)]
    pub created_at_us: i64,
    pub binding_id: String,
    pub workspace: String,
    pub owner: String,
    pub channel: String,
    pub thread_ts: String,
    pub text: String,
    pub destination: String,
    pub request_ids: Vec<String>,
    pub published: bool,
    pub state: SlackReturnState,
    pub notification_receipt: Option<String>,
    pub failure: Option<String>,
    pub product_id: Option<String>,
}

fn request_row(r: &sqlx::sqlite::SqliteRow) -> Result<SlackRequest, sqlx::Error> {
    Ok(SlackRequest {
        request_id: r.try_get("request_id")?,
        binding_id: r.try_get("binding_id")?,
        creation_id: r.try_get("creation_id")?,
        input_id: r.try_get("input_id")?,
        product_id: r.try_get("product_id")?,
        conversation_available: r.try_get("conversation_available")?,
        transcript_id: r.try_get("transcript_id")?,
        admitted: r.try_get::<String, _>("admission_state")? == "admitted",
        accepted_at_us: r.try_get("accepted_at_unix_micros")?,
        eyes_confirmed: r.try_get("eyes_confirmed")?,
        eyes_failure: r.try_get("eyes_failure")?,
        failure: r.try_get("failure")?,
        question_id: r.try_get("question_id")?,
        question_transcript: r.try_get("question_transcript")?,
        source: SlackSource {
            trigger_kind: SlackTriggerKind::decode(&r.try_get::<String, _>("trigger_kind")?)?,
            workspace: r.try_get("workspace")?,
            owner: r.try_get("owner")?,
            channel: r.try_get("channel")?,
            message_ts: r.try_get("message_ts")?,
            thread_ts: r.try_get("thread_ts")?,
            source_url: r.try_get("source_url")?,
            instruction: r.try_get("instruction")?,
            context: r.try_get("context")?,
            observed_at_unix_micros: r.try_get("observed_at_unix_micros")?,
            kind: if r.try_get::<String, _>("kind")? == "explicit" {
                SlackRequestKind::Explicit
            } else {
                SlackRequestKind::Briefing
            },
        },
    })
}
impl Database {
    pub async fn set_slack_configuration(&self, value: &str) -> DbResult<()> {
        let config: serde_json::Value =
            serde_json::from_str(value).map_err(|e| DbError::Serialization(e.to_string()))?;
        let workspace = config
            .get("workspace")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| DbError::Serialization("Slack workspace is required".into()))?;
        let owner = config
            .get("owner")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| DbError::Serialization("Slack owner is required".into()))?;
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let prior: Option<String> =
            sqlx::query_scalar("SELECT value FROM app_settings WHERE key='slack_gateway'")
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(prior) = prior {
            let prior: serde_json::Value =
                serde_json::from_str(&prior).map_err(|e| DbError::Serialization(e.to_string()))?;
            if prior.get("workspace") == config.get("workspace")
                && prior.get("owner") == config.get("owner")
                && prior.get("timezone") != config.get("timezone")
            {
                let initializing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slack_gateway_accounts account WHERE workspace=?1 AND owner=?2 AND baseline_complete=0 AND EXISTS(SELECT 1 FROM slack_gateway_discovery scan WHERE scan.workspace=account.workspace AND scan.owner=account.owner))").bind(workspace).bind(owner).fetch_one(&mut *tx).await?;
                if initializing {
                    return Err(DbError::Serialization(
                        "Finish Slack initialization before changing its timezone".into(),
                    ));
                }
            }
        }
        let outstanding: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM slack_gateway_requests r WHERE r.kind<>'baseline' AND (r.workspace<>?1 OR r.owner<>?2) AND (
                r.admission_state='accepted'
                OR (r.kind='briefing' AND r.admission_state='admitted' AND NOT EXISTS(SELECT 1 FROM slack_gateway_native_outcomes e WHERE e.request_id=r.request_id AND e.kind IN ('Completed','Cancelled','Failed') AND e.context_exhausted=0 AND e.processed=1))
                OR (r.kind='explicit' AND NOT EXISTS(SELECT 1 FROM slack_gateway_return_coverage c JOIN slack_gateway_returns o USING(return_id) WHERE c.request_id=r.request_id AND o.destination='thread' AND o.state IN ('confirmed','failed')))
            )) OR EXISTS(SELECT 1 FROM slack_gateway_returns o JOIN slack_gateway_bindings b USING(binding_id) WHERE (b.workspace<>?1 OR b.owner<>?2) AND o.destination='thread' AND o.state NOT IN ('confirmed','failed'))"
        ).bind(workspace).bind(owner).fetch_one(&mut *tx).await?;
        if outstanding {
            return Err(DbError::Serialization("Cannot change the Slack account while accepted work or return delivery is outstanding; finish that work using its original account first".into()));
        }
        sqlx::query("UPDATE slack_gateway_returns SET state='failed',failure='Private notice skipped because the configured account changed' WHERE destination='private' AND state IN ('pending','known_unsent') AND binding_id IN (SELECT binding_id FROM slack_gateway_bindings WHERE workspace<>?1 OR owner<>?2)")
            .bind(workspace).bind(owner).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO app_settings(key,value) VALUES ('slack_gateway',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(value).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    #[must_use]
    pub fn slack_gateway_wakeup(&self) -> std::sync::Arc<tokio::sync::Notify> {
        self.slack_gateway_wakeup.clone()
    }
    pub async fn slack_source_recorded(
        &self,
        workspace: &str,
        owner: &str,
        channel: &str,
        ts: &str,
    ) -> DbResult<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slack_gateway_requests WHERE workspace=?1 AND owner=?2 AND channel=?3 AND message_ts=?4)")
            .bind(workspace).bind(owner).bind(channel).bind(ts).fetch_one(self.pool()).await?)
    }
    pub async fn slack_baseline_complete(&self, workspace: &str, owner: &str) -> DbResult<bool> {
        sqlx::query("INSERT INTO slack_gateway_accounts(workspace,owner) VALUES (?1,?2) ON CONFLICT DO NOTHING").bind(workspace).bind(owner).execute(self.pool()).await?;
        Ok(sqlx::query_scalar(
            "SELECT baseline_complete FROM slack_gateway_accounts WHERE workspace=?1 AND owner=?2",
        )
        .bind(workspace)
        .bind(owner)
        .fetch_one(self.pool())
        .await?)
    }
    pub async fn slack_scan_status(
        &self,
        workspace: &str,
        owner: &str,
        complete: bool,
        status: &str,
        error: Option<&str>,
    ) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_accounts SET baseline_complete=MAX(baseline_complete,?3),status=?4,last_error=?5,last_scan_at_unix_micros=CASE WHEN ?3 THEN ?6 ELSE last_scan_at_unix_micros END WHERE workspace=?1 AND owner=?2")
            .bind(workspace).bind(owner).bind(complete).bind(status).bind(error).bind(chrono::Utc::now().timestamp_micros()).execute(self.pool()).await?;
        Ok(())
    }
    pub async fn slack_observe_source(
        &self,
        workspace: &str,
        owner: &str,
        channel: &str,
        message_ts: &str,
        observed_at: i64,
    ) -> DbResult<()> {
        sqlx::query("INSERT INTO slack_gateway_observations(workspace,owner,channel,message_ts,first_observed_at_unix_micros) VALUES (?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING")
            .bind(workspace).bind(owner).bind(channel).bind(message_ts).bind(observed_at).execute(self.pool()).await?;
        Ok(())
    }
    pub async fn slack_forget_observation(
        &self,
        workspace: &str,
        owner: &str,
        channel: &str,
        message_ts: &str,
    ) -> DbResult<()> {
        sqlx::query("DELETE FROM slack_gateway_observations WHERE workspace=?1 AND owner=?2 AND channel=?3 AND message_ts=?4")
            .bind(workspace).bind(owner).bind(channel).bind(message_ts).execute(self.pool()).await?;
        Ok(())
    }
    /// Atomically reserve the stable binding and its one immutable source request.
    pub async fn accept_slack_source(
        &self,
        source: &SlackSource,
        baseline: bool,
    ) -> DbResult<SlackSourceAcceptance> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        // Settings are read as one aggregate under the acceptance transaction.
        // A concurrent discovery disable cannot cross the durable acceptance point.
        let setting: Option<String> =
            sqlx::query_scalar("SELECT value FROM app_settings WHERE key='slack_gateway'")
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(value) = setting {
            let value: serde_json::Value =
                serde_json::from_str(&value).map_err(|e| DbError::Serialization(e.to_string()))?;
            if value.get("enabled").and_then(serde_json::Value::as_bool) != Some(true)
                || value.get("workspace").and_then(serde_json::Value::as_str)
                    != Some(source.workspace.as_str())
                || value.get("owner").and_then(serde_json::Value::as_str)
                    != Some(source.owner.as_str())
            {
                tx.rollback().await?;
                return Ok(SlackSourceAcceptance::Inactive);
            }
        }
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slack_gateway_requests WHERE workspace=?1 AND owner=?2 AND channel=?3 AND message_ts=?4)").bind(&source.workspace).bind(&source.owner).bind(&source.channel).bind(&source.message_ts).fetch_one(&mut *tx).await?;
        if exists {
            tx.rollback().await?;
            return Ok(SlackSourceAcceptance::Recorded);
        }
        if source.trigger_kind == SlackTriggerKind::UnknownHistorical {
            return Err(DbError::Serialization(
                "New Slack sources require verified trigger provenance".into(),
            ));
        }
        let request = uuid::Uuid::new_v4().to_string();
        let binding = if source.kind == SlackRequestKind::Explicit && !baseline {
            sqlx::query_scalar::<_,String>("SELECT binding_id FROM slack_gateway_bindings WHERE workspace=?1 AND owner=?2 AND channel=?3 AND thread_ts=?4 AND kind='explicit'")
                .bind(&source.workspace).bind(&source.owner).bind(&source.channel).bind(&source.thread_ts).fetch_optional(&mut *tx).await?.unwrap_or_else(||uuid::Uuid::new_v4().to_string())
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        if !baseline {
            sqlx::query("INSERT INTO slack_gateway_bindings(binding_id,workspace,owner,channel,thread_ts,kind,creation_id) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(binding_id) DO NOTHING")
                .bind(&binding).bind(&source.workspace).bind(&source.owner).bind(&source.channel).bind(&source.thread_ts).bind(source.kind.as_str()).bind(uuid::Uuid::new_v4().to_string()).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO slack_gateway_requests(request_id,workspace,owner,channel,message_ts,thread_ts,kind,instruction,source_url,binding_id,accepted_at_unix_micros,input_id,context,observed_at_unix_micros,trigger_kind) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,COALESCE((SELECT first_observed_at_unix_micros FROM slack_gateway_observations WHERE workspace=?2 AND owner=?3 AND channel=?4 AND message_ts=?5),?14),?15)")
            .bind(&request).bind(&source.workspace).bind(&source.owner).bind(&source.channel).bind(&source.message_ts).bind(&source.thread_ts).bind(if baseline {"baseline"}else{source.kind.as_str()}).bind(&source.instruction).bind(&source.source_url).bind(if baseline {None}else{Some(&binding)}).bind(chrono::Utc::now().timestamp_micros()).bind(uuid::Uuid::new_v4().to_string()).bind(&source.context).bind(source.observed_at_unix_micros).bind(source.trigger_kind.as_str()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM slack_gateway_observations WHERE workspace=?1 AND owner=?2 AND channel=?3 AND message_ts=?4")
            .bind(&source.workspace).bind(&source.owner).bind(&source.channel).bind(&source.message_ts).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(SlackSourceAcceptance::Recorded)
    }
    pub async fn slack_requests(
        &self,
        workspace: &str,
        owner: &str,
    ) -> DbResult<Vec<SlackRequest>> {
        // Interpolation is limited to native routing's static predicate; inputs stay bound.
        Ok(sqlx::query(sqlx::AssertSqlSafe(format!("SELECT r.request_id,r.binding_id,r.input_id,r.transcript_id,r.admission_state,r.accepted_at_unix_micros,r.eyes_confirmed,r.eyes_failure,r.failure,r.question_id,r.question_transcript,r.trigger_kind,r.workspace,r.owner,r.channel,r.message_ts,r.thread_ts,r.source_url,r.instruction,r.context,r.observed_at_unix_micros,r.kind,b.creation_id,b.product_id, EXISTS(SELECT 1 FROM product_conversations p JOIN conversations c ON c.product_conversation_id=p.id WHERE p.id=b.product_id AND {ORDINARY_PRODUCT_ROUTE_PREDICATE}) AS conversation_available FROM slack_gateway_requests r JOIN slack_gateway_bindings b USING(binding_id) WHERE r.workspace=?1 AND r.owner=?2 ORDER BY r.accepted_at_unix_micros")))
            .bind(workspace).bind(owner).try_map(|r| request_row(&r)).fetch_all(self.pool()).await?)
    }
    pub async fn slack_bind_product(&self, binding: &str, product: &str) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_bindings SET product_id=?2 WHERE binding_id=?1 AND (product_id IS NULL OR product_id=?2)").bind(binding).bind(product).execute(self.pool()).await?;
        Ok(())
    }
    pub async fn slack_mark_admitted(&self, request: &str, transcript: &str) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_requests SET admission_state='admitted',transcript_id=?2 WHERE request_id=?1 AND admission_state='accepted'").bind(request).bind(transcript).execute(self.pool()).await?;
        Ok(())
    }
    pub async fn slack_begin_eyes_attempt(&self, request: &str) -> DbResult<bool> {
        Ok(sqlx::query("UPDATE slack_gateway_requests SET eyes_failure=?2 WHERE request_id=?1 AND kind<>'baseline' AND eyes_confirmed=0 AND eyes_failure IS NULL")
            .bind(request).bind(EYES_OUTCOME_UNKNOWN).execute(self.pool()).await?.rows_affected()==1)
    }
    pub async fn slack_eyes_confirmed(&self, request: &str) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_requests SET eyes_failure=NULL,eyes_confirmed=1 WHERE request_id=?1 AND (eyes_failure IS NULL OR eyes_failure=?2)")
            .bind(request)
            .bind(EYES_OUTCOME_UNKNOWN)
            .execute(self.pool())
            .await?;
        Ok(())
    }
    pub async fn slack_eyes_unavailable(&self, request: &str, reason: &str) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_requests SET eyes_failure=?2 WHERE request_id=?1 AND eyes_confirmed=0 AND (eyes_failure IS NULL OR eyes_failure=?3)").bind(request).bind(reason).bind(EYES_OUTCOME_UNKNOWN).execute(self.pool()).await?;
        Ok(())
    }
    /// One return per native occurrence; coverage is checked and persisted atomically.
    pub async fn queue_slack_reply(
        &self,
        product: &str,
        request_ids: &[String],
        transcript: &str,
        source: &phoenix_core::domain::db_schema::SourceToolCall,
        text: &str,
    ) -> DbResult<String> {
        let occurrence = format!("{transcript}:{}:{}", source.message_id, source.tool_use_id);
        let source_message = source.message_id.as_str();
        if request_ids.is_empty()
            || request_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != request_ids.len()
            || text.trim().is_empty()
        {
            return Err(DbError::Serialization(
                "response requires text and request coverage".into(),
            ));
        }
        let mut tx = self.pool().begin().await?;
        let existing=sqlx::query("SELECT o.return_id,o.text,o.native_message_id FROM slack_gateway_returns o JOIN slack_gateway_bindings b USING(binding_id) WHERE b.product_id=?1 AND o.occurrence_key=?2 AND o.destination='thread'")
            .bind(product).bind(&occurrence).fetch_optional(&mut *tx).await?;
        if let Some(existing) = existing {
            let id: String = existing.try_get("return_id")?;
            let stored:Vec<String>=sqlx::query_scalar("SELECT request_id FROM slack_gateway_return_coverage WHERE return_id=?1 ORDER BY request_id").bind(&id).fetch_all(&mut *tx).await?;
            let mut expected = request_ids.to_vec();
            expected.sort();
            if stored != expected
                || existing.try_get::<String, _>("text")? != text
                || existing
                    .try_get::<Option<String>, _>("native_message_id")?
                    .as_deref()
                    != Some(source_message)
            {
                return Err(DbError::Serialization(
                    "Reply identity reused with different source, coverage, or text".into(),
                ));
            }
            tx.commit().await?;
            self.slack_gateway_wakeup.notify_one();
            return Ok(id);
        }
        let eligible = active_reply_requests(&mut tx, product, transcript, source).await?;
        if request_ids.iter().any(|id| !eligible.contains(id)) {
            return Err(DbError::Serialization("Reply coverage is outside this native execution; a private follow-up does not inherit a Slack return address".into()));
        }
        let mut binding: Option<String> = None;
        for request in request_ids {
            let row=sqlx::query("SELECT r.binding_id,b.product_id,r.kind,r.admission_state FROM slack_gateway_requests r JOIN slack_gateway_bindings b USING(binding_id) WHERE request_id=?1").bind(request).fetch_one(&mut *tx).await?;
            let candidate: String = row.try_get("binding_id")?;
            if row.try_get::<Option<String>, _>("product_id")?.as_deref() != Some(product)
                || row.try_get::<String, _>("kind")? != "explicit"
                || binding.as_ref().is_some_and(|b| *b != candidate)
            {
                return Err(DbError::Serialization(
                    "response coverage does not belong to the executing product and thread".into(),
                ));
            }
            let native:Option<String>=sqlx::query_scalar("SELECT m.conversation_id FROM messages m JOIN conversations c ON c.id=m.conversation_id WHERE m.origin_slack_request_id=?1 AND c.product_conversation_id=?2 LIMIT 1").bind(request).bind(product).fetch_optional(&mut *tx).await?;
            let Some(native) = native else {
                return Err(DbError::Serialization(
                    "No persisted native input for covered request".into(),
                ));
            };
            sqlx::query("UPDATE slack_gateway_requests SET admission_state='admitted',transcript_id=?2 WHERE request_id=?1 AND admission_state='accepted'").bind(request).bind(native).execute(&mut *tx).await?;
            binding = Some(candidate);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let destination = "thread";
        let binding =
            binding.ok_or_else(|| DbError::Serialization("missing response binding".into()))?;
        for request in request_ids {
            let answered: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slack_gateway_return_coverage c JOIN slack_gateway_returns o USING(return_id) WHERE c.request_id=?1 AND NOT (o.binding_id=?2 AND o.occurrence_key=?3 AND o.destination=?4))")
                .bind(request).bind(&binding).bind(&occurrence).bind(destination).fetch_one(&mut *tx).await?;
            if answered {
                return Err(DbError::Serialization(
                    "Request already has a selected answer; cover only unanswered request IDs"
                        .into(),
                ));
            }
        }
        let inserted=sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,native_message_id,created_at_unix_micros) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(binding_id,occurrence_key,destination) DO NOTHING")
            .bind(&id).bind(&binding).bind(&occurrence).bind(destination).bind(text).bind(source_message).bind(chrono::Utc::now().timestamp_micros()).execute(&mut *tx).await?;
        let actual:String=sqlx::query_scalar("SELECT return_id FROM slack_gateway_returns WHERE binding_id=?1 AND occurrence_key=?2 AND destination=?3 AND text=?4").bind(&binding).bind(&occurrence).bind(destination).bind(text).fetch_one(&mut *tx).await?;
        if inserted.rows_affected() > 0 {
            for request in request_ids {
                sqlx::query("INSERT INTO slack_gateway_return_coverage(return_id,request_id) VALUES (?1,?2)").bind(&actual).bind(request).execute(&mut *tx).await?;
            }
        } else {
            let stored:Vec<String>=sqlx::query_scalar("SELECT request_id FROM slack_gateway_return_coverage WHERE return_id=?1 ORDER BY request_id").bind(&actual).fetch_all(&mut *tx).await?;
            let mut expected = request_ids.to_vec();
            expected.sort();
            expected.dedup();
            if stored != expected {
                return Err(DbError::Serialization(
                    "return identity reused with different coverage".into(),
                ));
            }
        }
        tx.commit().await?;
        self.slack_gateway_wakeup.notify_one();
        Ok(actual)
    }
    pub async fn slack_returns(&self, workspace: &str, owner: &str) -> DbResult<Vec<SlackReturn>> {
        Ok(sqlx::query(sqlx::AssertSqlSafe(format!("SELECT o.return_id,o.created_at_unix_micros,o.binding_id,o.text,o.destination,o.state,o.notification_receipt,o.failure,b.workspace,b.owner,b.channel,b.thread_ts,b.product_id, EXISTS(SELECT 1 FROM product_conversations p JOIN conversations c ON c.product_conversation_id=p.id WHERE p.id=b.product_id AND {ORDINARY_PRODUCT_ROUTE_PREDICATE}) AS published, (SELECT json_group_array(request_id) FROM slack_gateway_return_coverage c WHERE c.return_id=o.return_id) AS request_ids FROM slack_gateway_returns o JOIN slack_gateway_bindings b USING(binding_id) WHERE b.workspace=?1 AND b.owner=?2 ORDER BY o.created_at_unix_micros")))
            .bind(workspace).bind(owner).try_map(|r:sqlx::sqlite::SqliteRow|Ok(SlackReturn{return_id:r.try_get("return_id")?,created_at_us:r.try_get("created_at_unix_micros")?,binding_id:r.try_get("binding_id")?,workspace:r.try_get("workspace")?,owner:r.try_get("owner")?,channel:r.try_get("channel")?,thread_ts:r.try_get("thread_ts")?,text:r.try_get("text")?,destination:r.try_get("destination")?,published:r.try_get("published")?,request_ids:serde_json::from_str(&r.try_get::<String,_>("request_ids")?).map_err(|e|sqlx::Error::Decode(e.into()))?,state:SlackReturnState::decode(&r.try_get::<String,_>("state")?)?,notification_receipt:r.try_get("notification_receipt")?,failure:r.try_get("failure")?,product_id:r.try_get("product_id")?})).fetch_all(self.pool()).await?)
    }
    pub async fn slack_begin_return(&self, id: &str, attempt: &str) -> DbResult<bool> {
        Ok(sqlx::query("UPDATE slack_gateway_returns SET state='attempting',attempt_id=?2 WHERE return_id=?1 AND destination='thread' AND state IN ('pending','known_unsent') AND (response_kind<>'clarification' OR NOT EXISTS(SELECT 1 FROM slack_gateway_questions q WHERE q.return_id=?1))").bind(id).bind(attempt).execute(self.pool()).await?.rows_affected()==1)
    }
    pub async fn slack_begin_notification_return(
        &self,
        id: &str,
        attempt: &str,
        command: &[String],
        url: &str,
    ) -> DbResult<bool> {
        let frozen =
            serde_json::to_string(command).map_err(|e| DbError::Serialization(e.to_string()))?;
        Ok(sqlx::query("UPDATE slack_gateway_returns SET state='attempting',attempt_id=?2,notification_command=?3,notification_url=?4 WHERE return_id=?1 AND destination='private' AND state='pending' AND attempt_id IS NULL AND notification_command IS NULL AND notification_url IS NULL")
            .bind(id).bind(attempt).bind(frozen).bind(url).execute(self.pool()).await?.rows_affected()==1)
    }
    pub async fn slack_skip_unconfigured_notification(&self, id: &str) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_returns SET state='failed',failure='Private notification command is not configured; no automatic retry' WHERE return_id=?1 AND destination='private' AND state='pending' AND attempt_id IS NULL")
            .bind(id).execute(self.pool()).await?;
        Ok(())
    }
    pub async fn slack_finish_return(
        &self,
        id: &str,
        attempt: &str,
        state: SlackReturnState,
        proof: Option<&str>,
        receipt: Option<&str>,
        error: Option<&str>,
    ) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_returns SET state=CASE WHEN destination='private' AND ?3='known_unsent' THEN 'failed' ELSE ?3 END,delivery_proof=?4,notification_receipt=?5,failure=?6 WHERE return_id=?1 AND attempt_id=?2 AND state='attempting'").bind(id).bind(attempt).bind(state.as_str()).bind(proof).bind(receipt).bind(error).execute(self.pool()).await?;
        Ok(())
    }
    pub async fn slack_recover_returns(&self) -> DbResult<()> {
        sqlx::query("UPDATE slack_gateway_returns SET state='uncertain',failure='Interrupted external attempt: delivery not established' WHERE state='attempting'").execute(self.pool()).await?;
        sqlx::query("UPDATE slack_gateway_returns SET state='failed',failure='Private notification was not sent; automatic retries are disabled' WHERE destination='private' AND (state='known_unsent' OR (state='pending' AND (attempt_id IS NOT NULL OR notification_command IS NOT NULL OR notification_url IS NOT NULL)))")
            .execute(self.pool()).await?;
        Ok(())
    }
    pub async fn slack_fail_return_with_notice(
        &self,
        id: &str,
        attempt: &str,
        reason: &str,
    ) -> DbResult<()> {
        let mut tx = self.pool().begin().await?;
        let changed=sqlx::query("UPDATE slack_gateway_returns SET state='failed',failure=?3 WHERE return_id=?1 AND attempt_id=?2 AND state='attempting' AND destination='thread'").bind(id).bind(attempt).bind(reason).execute(&mut *tx).await?;
        if changed.rows_affected() == 1 {
            let notice = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,response_kind,created_at_unix_micros) SELECT ?2,binding_id,?3,'private','Phoenix could not reply in the source Slack thread; open Phoenix for the recorded delivery failure.','notice',?4 FROM slack_gateway_returns WHERE return_id=?1")
                .bind(id).bind(&notice).bind(format!("delivery-failed:{id}")).bind(chrono::Utc::now().timestamp_micros()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO slack_gateway_return_coverage(return_id,request_id) SELECT ?2,request_id FROM slack_gateway_return_coverage WHERE return_id=?1").bind(id).bind(&notice).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Capture exact native evidence inside the native persistence transaction.
/// This is not a second execution state machine or a transcript-tail watcher.
pub(crate) async fn recover_completed_slack_outcomes_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    transcript: Option<&str>,
) -> DbResult<()> {
    let completed: Vec<String> = sqlx::query_scalar("SELECT c.id FROM conversations c JOIN steering_execution_occurrences o ON o.conversation_id=c.id JOIN messages m ON m.conversation_id=c.id AND m.message_id=o.message_id WHERE (?1 IS NULL OR c.id=?1) AND c.state_kind IN ('idle','terminal','completed') AND EXISTS(SELECT 1 FROM messages candidate WHERE candidate.conversation_id=c.id AND candidate.origin_slack_request_id IS NOT NULL) AND o.source_kind IN ('steering','seeded_fork','interaction_response') AND EXISTS(SELECT 1 FROM messages answer WHERE answer.conversation_id=c.id AND answer.message_type='agent' AND answer.sequence_id>m.sequence_id) AND NOT EXISTS(SELECT 1 FROM durable_turns t WHERE t.conversation_id=c.id AND t.owns_conversation=1)")
        .bind(transcript).fetch_all(&mut **tx).await?;
    for transcript in completed {
        crate::record_initial_execution_outcome_tx(tx, &transcript, &super::ConvState::Idle)
            .await?;
    }
    Ok(())
}

pub(crate) async fn record_slack_deleted_product_conn(
    connection: &mut sqlx::SqliteConnection,
    product: &str,
    deleted_transcript: &str,
) -> DbResult<()> {
    sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id)
        SELECT r.request_id,'deleted:' || ?1,'Cancelled',?2
        FROM slack_gateway_requests r JOIN slack_gateway_bindings b USING(binding_id)
        WHERE b.product_id=?1 AND r.admission_state='admitted'
          AND NOT EXISTS(SELECT 1 FROM conversations c WHERE c.product_conversation_id=?1 AND c.runtime_role='user' AND c.parent_conversation_id IS NULL AND c.state_kind<>'handed_off')
          AND NOT EXISTS(SELECT 1 FROM slack_gateway_native_outcomes e WHERE e.request_id=r.request_id AND e.kind IN ('Completed','Cancelled','Failed') AND e.context_exhausted=0)
        ON CONFLICT DO NOTHING")
        .bind(product).bind(deleted_transcript).execute(connection).await?;
    Ok(())
}

pub(crate) async fn record_slack_native_outcome_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    transcript: &str,
    source: &str,
    occurrence: &str,
    kind: &str,
    context_exhausted: bool,
) -> DbResult<()> {
    if !matches!(kind, "Completed" | "Cancelled" | "Failed") {
        return Ok(());
    }
    let mut requests: std::collections::BTreeSet<_> =
        requests_for_native_input(tx, transcript, source)
            .await?
            .into_iter()
            .collect();
    // A later steering drain replaces the native anchor. Every materialized,
    // dequeued Slack input still owes its terminal explanation until an outcome
    // is captured. This broadening is only for generic terminal fallback, never
    // permission to publish a private question or a model-selected answer.
    let consumed: Vec<String> = sqlx::query_scalar("SELECT m.origin_slack_request_id FROM messages m
        JOIN steering_acceptance_receipts receipt ON receipt.conversation_id=m.conversation_id AND receipt.message_id=m.message_id
        WHERE m.conversation_id=?1 AND m.origin_slack_request_id IS NOT NULL
          AND NOT EXISTS(SELECT 1 FROM steering_messages pending WHERE pending.conversation_id=m.conversation_id AND pending.message_id=m.message_id)
          AND NOT EXISTS(SELECT 1 FROM slack_gateway_native_outcomes outcome WHERE outcome.request_id=m.origin_slack_request_id AND outcome.kind IN ('Completed','Cancelled','Failed'))")
        .bind(transcript).fetch_all(&mut **tx).await?;
    requests.extend(consumed);
    for request in requests {
        sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id,context_exhausted) SELECT ?1,?2,?3,?4,?5 FROM conversations WHERE id=?4 AND (?3!='Completed' OR state_kind IN ('idle','terminal','completed')) ON CONFLICT DO NOTHING")
            .bind(request).bind(occurrence).bind(if context_exhausted {"Failed"}else{kind}).bind(transcript).bind(context_exhausted).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn active_reply_requests(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    product: &str,
    transcript: &str,
    source: &phoenix_core::domain::db_schema::SourceToolCall,
) -> DbResult<std::collections::BTreeSet<String>> {
    let state: String = sqlx::query_scalar(
        "SELECT state FROM conversations WHERE id=?1 AND product_conversation_id=?2",
    )
    .bind(transcript)
    .bind(product)
    .fetch_one(&mut **tx)
    .await?;
    let state: super::ConvState =
        serde_json::from_str(&state).map_err(|e| DbError::Serialization(e.to_string()))?;
    let super::ConvState::ToolExecuting {
        current_tool,
        assistant_message,
        ..
    } = state
    else {
        return Err(DbError::Serialization(
            "Reply selection is not the current native tool execution".into(),
        ));
    };
    if current_tool.id != source.tool_use_id
        || current_tool.name() != "slack_gateway_reply"
        || assistant_message.message_id != source.message_id
    {
        return Err(DbError::Serialization(
            "Reply source is not the current native tool occurrence".into(),
        ));
    }
    let inputs = current_execution_inputs(tx, transcript).await?;
    let mut requests = std::collections::BTreeSet::new();
    for input in inputs {
        requests.extend(requests_for_native_input(tx, transcript, &input).await?);
    }
    Ok(requests)
}

async fn requests_for_native_input(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    transcript: &str,
    message: &str,
) -> DbResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT origin_slack_request_id FROM messages WHERE conversation_id=?1 AND message_id=?2 AND origin_slack_request_id IS NOT NULL
        UNION SELECT member.origin_slack_request_id FROM messages anchor
        JOIN messages member ON member.conversation_id=anchor.conversation_id
        JOIN steering_acceptance_receipts receipt ON receipt.conversation_id=member.conversation_id AND receipt.message_id=member.message_id
        WHERE anchor.conversation_id=?1 AND anchor.message_id=?2 AND member.origin_slack_request_id IS NOT NULL
          AND member.sequence_id>=anchor.sequence_id
          AND member.sequence_id<COALESCE((SELECT MIN(sequence_id) FROM messages answer WHERE answer.conversation_id=?1 AND answer.message_type='agent' AND answer.sequence_id>anchor.sequence_id),9223372036854775807)
          AND NOT EXISTS(SELECT 1 FROM steering_messages pending WHERE pending.conversation_id=member.conversation_id AND pending.message_id=member.message_id)
        UNION SELECT e.request_id FROM slack_gateway_native_outcomes e
        JOIN completed_continuation_handoffs h ON h.predecessor_conversation_id=e.transcript_id
        JOIN automatic_continuation_admissions a ON a.predecessor_conversation_id=h.predecessor_conversation_id
        WHERE h.successor_conversation_id=?1 AND h.accepted_successor_message_id=?2 AND h.opening_authority='generated_predecessor_context'
          AND e.context_exhausted=1 AND a.phase NOT IN ('failed','superseded')
          AND NOT EXISTS(SELECT 1 FROM slack_gateway_return_coverage rc JOIN slack_gateway_returns o USING(return_id) WHERE rc.request_id=e.request_id)")
        .bind(transcript).bind(message).fetch_all(&mut **tx).await?)
}

async fn current_execution_inputs(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    transcript: &str,
) -> DbResult<Vec<String>> {
    // A steering/interaction occurrence supersedes the original direct input
    // even while its workflow is still settling. Private follow-ups cannot
    // inherit its publication authority through a lagging durable-turn owner.
    Ok(sqlx::query_scalar("SELECT message_id FROM steering_execution_occurrences WHERE conversation_id=?1
        UNION SELECT canonical_message_id FROM durable_turns WHERE conversation_id=?1 AND owns_conversation=1 AND canonical_message_id IS NOT NULL
          AND NOT EXISTS(SELECT 1 FROM steering_execution_occurrences WHERE conversation_id=?1)")
        .bind(transcript).fetch_all(&mut **tx).await?)
}
pub(crate) async fn record_slack_question_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    transcript: &str,
    state: &super::ConvState,
) -> DbResult<()> {
    let super::ConvState::AwaitingUserResponse {
        request_authority, ..
    } = state
    else {
        return Ok(());
    };
    let Some(question) = request_authority.request_id() else {
        return Ok(());
    };
    let inputs = current_execution_inputs(tx, transcript).await?;
    for input in inputs {
        for request in requests_for_native_input(tx, transcript, &input).await? {
            sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id) VALUES (?1,?2,'question',?3) ON CONFLICT DO NOTHING")
                .bind(request).bind(question.to_string()).bind(transcript).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

async fn replace_legacy_slack_clarifications_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    phoenix_origin: &str,
) -> DbResult<()> {
    let pending = sqlx::query("SELECT return_id,binding_id FROM slack_gateway_returns o WHERE destination='thread' AND response_kind='clarification' AND state IN ('pending','known_unsent') AND EXISTS(SELECT 1 FROM slack_gateway_questions q WHERE q.return_id=o.return_id)")
        .fetch_all(&mut **tx).await?;
    for output in pending {
        let old: String = output.try_get("return_id")?;
        let binding: String = output.try_get("binding_id")?;
        let replacement = uuid::Uuid::new_v4().to_string();
        let text = slack_thread_response_tx(
            tx,
            &binding,
            &replacement,
            CLARIFICATION_HANDOFF,
            phoenix_origin,
        )
        .await?;
        sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,response_kind,created_at_unix_micros) VALUES (?1,?2,?3,'thread',?4,'clarification',?5)")
            .bind(&replacement).bind(&binding).bind(format!("generic-handoff:{old}")).bind(text).bind(chrono::Utc::now().timestamp_micros()).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO slack_gateway_return_coverage(return_id,request_id) SELECT ?2,request_id FROM slack_gateway_return_coverage WHERE return_id=?1")
            .bind(&old).bind(&replacement).execute(&mut **tx).await?;
        sqlx::query("UPDATE slack_gateway_returns SET state='failed',failure='Question-text publication retired; generic handoff queued' WHERE return_id=?1")
            .bind(&old).execute(&mut **tx).await?;
    }
    Ok(())
}

impl Database {
    /// Materialize replies/notices from transactionally captured native occurrences.
    pub async fn process_slack_native_outcomes(&self, phoenix_origin: &str) -> DbResult<()> {
        let mut tx = self.pool().begin().await?;
        replace_legacy_slack_clarifications_tx(&mut tx, phoenix_origin).await?;
        recover_completed_slack_outcomes_tx(&mut tx, None).await?;
        let events=sqlx::query("SELECT e.request_id,e.occurrence_key,e.kind,e.transcript_id,e.context_exhausted,r.binding_id,r.kind AS request_kind,r.source_url FROM slack_gateway_native_outcomes e JOIN slack_gateway_requests r USING(request_id) WHERE e.processed=0").fetch_all(&mut *tx).await?;
        for event in events {
            let request: String = event.try_get("request_id")?;
            let occurrence: String = event.try_get("occurrence_key")?;
            let kind: String = event.try_get("kind")?;
            if event.try_get::<bool, _>("context_exhausted")? {
                let lifecycle=sqlx::query("SELECT c.state_kind,p.auto_continue_on_context_exhaustion,a.phase FROM conversations c JOIN product_conversations p ON p.id=c.product_conversation_id LEFT JOIN automatic_continuation_admissions a ON a.predecessor_conversation_id=c.id WHERE c.id=?1")
                    .bind(event.try_get::<String,_>("transcript_id")?).fetch_optional(&mut *tx).await?;
                if let Some(lifecycle) = lifecycle {
                    let automatic =
                        lifecycle.try_get::<bool, _>("auto_continue_on_context_exhaustion")?;
                    let phase: Option<String> = lifecycle.try_get("phase")?;
                    if phase.as_deref() == Some("message_settled") {
                        sqlx::query("UPDATE slack_gateway_native_outcomes SET processed=1 WHERE request_id=?1 AND occurrence_key=?2")
                        .bind(&request).bind(&occurrence).execute(&mut *tx).await?;
                        continue;
                    }
                    let failed = phase
                        .as_deref()
                        .is_some_and(|s| matches!(s, "failed" | "superseded"));
                    let state: String = lifecycle.try_get("state_kind")?;
                    if automatic
                        && !failed
                        && matches!(
                            state.as_str(),
                            "context_exhausted" | "awaiting_continuation" | "handed_off"
                        )
                    {
                        continue;
                    }
                }
            }
            let private = event.try_get::<String, _>("request_kind")? == "briefing";
            let already:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slack_gateway_return_coverage c JOIN slack_gateway_returns r USING(return_id) WHERE c.request_id=?1 AND (?2=0 OR r.occurrence_key=?3))").bind(&request).bind(private).bind(&occurrence).fetch_one(&mut *tx).await?;
            if !already {
                let question = kind == "question";
                let text = if occurrence.starts_with("deleted:") {
                    "Phoenix: this conversation was deleted; open gateway activity for the retained request and delivery status.".into()
                } else if private {
                    match kind.as_str() {
                        "Completed" => "Phoenix: your briefing is ready",
                        "question" => "Phoenix: your briefing needs your input",
                        _ => "Phoenix: your briefing stopped; open Phoenix for details",
                    }
                    .to_string()
                } else if question {
                    CLARIFICATION_HANDOFF.into()
                } else {
                    match kind.as_str(){"Completed"=>"Phoenix finished this request without selecting a Slack answer. Open its Phoenix conversation for details or send an explicit follow-up.","Cancelled"=>"Phoenix execution was cancelled. Send a new explicit message to request further work.",_=>"Phoenix execution stopped with an error. Open its Phoenix conversation for details."}.into()
                };
                let binding: String = event.try_get("binding_id")?;
                let destination = if private { "private" } else { "thread" };
                let id = uuid::Uuid::new_v4().to_string();
                let text = if private {
                    text
                } else {
                    slack_thread_response_tx(&mut tx, &binding, &id, &text, phoenix_origin).await?
                };
                sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,response_kind,created_at_unix_micros) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT DO NOTHING")
                    .bind(&id).bind(&binding).bind(&occurrence).bind(destination).bind(&text).bind(if question && !private {"clarification"}else if private {"notice"}else{"failure"}).bind(chrono::Utc::now().timestamp_micros()).execute(&mut *tx).await?;
                let id: String = sqlx::query_scalar("SELECT return_id FROM slack_gateway_returns WHERE binding_id=?1 AND occurrence_key=?2 AND destination=?3")
                    .bind(&binding).bind(&occurrence).bind(destination).fetch_one(&mut *tx).await?;
                sqlx::query("INSERT INTO slack_gateway_return_coverage(return_id,request_id) VALUES (?1,?2) ON CONFLICT DO NOTHING").bind(&id).bind(&request).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE slack_gateway_native_outcomes SET processed=1 WHERE request_id=?1 AND occurrence_key=?2").bind(&request).bind(&occurrence).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn fail_slack_before_admission(
        &self,
        request: &str,
        reason: &str,
        phoenix_origin: &str,
    ) -> DbResult<()> {
        let mut tx = self.pool().begin().await?;
        reject_slack_request_tx(&mut tx, request, reason, phoenix_origin).await?;
        tx.commit().await?;
        Ok(())
    }
}

impl Database {
    pub async fn is_gateway_slack_source(
        &self,
        workspace: &str,
        owner: &str,
        channel: &str,
        ts: &str,
        exact_author_body: Option<(&str, &str)>,
    ) -> DbResult<bool> {
        let rows=sqlx::query("SELECT o.text,o.delivery_proof,o.state FROM slack_gateway_returns o JOIN slack_gateway_bindings b USING(binding_id) WHERE b.workspace=?1 AND b.owner=?2 AND b.channel=?3 AND o.destination='thread' AND o.state NOT IN ('pending','known_unsent')").bind(workspace).bind(owner).bind(channel).fetch_all(self.pool()).await?;
        for row in rows {
            let proof: Option<String> = row.try_get("delivery_proof")?;
            if proof
                .as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .is_some_and(|v| {
                    v.get("message_ts").and_then(serde_json::Value::as_str) == Some(ts)
                })
            {
                return Ok(true);
            }
            let generated = format!("Phoenix: {}", row.try_get::<String, _>("text")?);
            let state: String = row.try_get("state")?;
            if matches!(state.as_str(), "attempting" | "uncertain")
                && exact_author_body
                    .is_some_and(|(author, text)| author == owner && text.trim() == generated)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

async fn slack_thread_response_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    binding: &str,
    return_id: &str,
    text: &str,
    phoenix_origin: &str,
) -> DbResult<String> {
    // Use the same static route predicate as native resolution, with bound identity.
    let product: Option<String> =
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT p.id FROM slack_gateway_bindings b JOIN product_conversations p ON p.id=b.product_id JOIN conversations c ON c.product_conversation_id=p.id WHERE b.binding_id=?1 AND {ORDINARY_PRODUCT_ROUTE_PREDICATE} LIMIT 1")))
            .bind(binding)
            .fetch_optional(&mut **tx)
            .await?;
    let origin = phoenix_origin.trim_end_matches('/');
    let url = product.map_or_else(
        || format!("{origin}/settings/slack#slack-return-{return_id}"),
        |product| format!("{origin}/c/{product}"),
    );
    Ok(format!("{text}\n\nOpen in Phoenix: {url}"))
}

async fn reject_slack_request_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request: &str,
    reason: &str,
    phoenix_origin: &str,
) -> DbResult<()> {
    let row=sqlx::query("SELECT binding_id,kind FROM slack_gateway_requests WHERE request_id=?1 AND admission_state='accepted'").bind(request).fetch_optional(&mut **tx).await?;
    if let Some(r) = row {
        sqlx::query("UPDATE slack_gateway_requests SET admission_state='rejected',failure=?2 WHERE request_id=?1").bind(request).bind(reason).execute(&mut **tx).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let binding: String = r.try_get("binding_id")?;
        let text = "Phoenix could not admit this request. Open Phoenix to answer any pending question or inspect the recorded reason; send a new Slack request after resolving it.";
        let text = if r.try_get::<String, _>("kind")? == "briefing" {
            text.to_string()
        } else {
            slack_thread_response_tx(tx, &binding, &id, text, phoenix_origin).await?
        };
        sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,response_kind,created_at_unix_micros) VALUES (?1,?2,?3,?4,?5,'failure',?6)").bind(&id).bind(r.try_get::<String,_>("binding_id")?).bind(format!("rejected:{request}")).bind(if r.try_get::<String,_>("kind")?=="briefing" {"private"}else{"thread"}).bind(text).bind(chrono::Utc::now().timestamp_micros()).execute(&mut **tx).await?;
        sqlx::query(
            "INSERT INTO slack_gateway_return_coverage(return_id,request_id) VALUES (?1,?2)",
        )
        .bind(&id)
        .bind(request)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn timezone_change_cannot_reinterpret_an_unfinished_baseline() {
        let db = Database::open_in_memory().await.unwrap();
        let original = serde_json::json!({"workspace":"example.slack.com","owner":"UOWNER","timezone":"UTC","enabled":true});
        db.set_slack_configuration(&original.to_string())
            .await
            .unwrap();
        db.slack_baseline_complete("example.slack.com", "UOWNER")
            .await
            .unwrap();
        let mut changed = original.clone();
        changed["timezone"] = serde_json::json!("America/New_York");
        // Inspection alone creates an account, but has no baseline boundary.
        db.set_slack_configuration(&changed.to_string())
            .await
            .unwrap();
        db.set_slack_configuration(&original.to_string())
            .await
            .unwrap();
        sqlx::query("INSERT INTO slack_gateway_discovery(workspace,owner,kind,after_date,cursor) VALUES ('example.slack.com','UOWNER','reaction','2026-10-19','saved')").execute(db.pool()).await.unwrap();
        assert!(db
            .set_slack_configuration(&changed.to_string())
            .await
            .is_err());
        assert_eq!(
            db.get_app_setting("slack_gateway").await.unwrap(),
            Some(original.to_string())
        );
        let mut paused = original;
        paused["enabled"] = serde_json::json!(false);
        db.set_slack_configuration(&paused.to_string())
            .await
            .unwrap();
        sqlx::query("UPDATE slack_gateway_accounts SET baseline_complete=1")
            .execute(db.pool())
            .await
            .unwrap();
        db.set_slack_configuration(&changed.to_string())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn unavailable_acknowledgment_is_terminal_without_discarding_work() {
        let db = Database::open_in_memory().await.unwrap();
        let source = source("100.000", SlackRequestKind::Briefing);
        let original =
            serde_json::json!({"workspace":source.workspace,"owner":source.owner,"enabled":true})
                .to_string();
        db.set_slack_configuration(&original).await.unwrap();
        db.accept_slack_source(&source, false).await.unwrap();
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        db.fail_slack_before_admission(
            &request.request_id,
            "fixture refusal",
            "https://phoenix.example",
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE slack_gateway_returns SET state='failed',failure='fixture terminal delivery'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let other =
            serde_json::json!({"workspace":"other.slack.com","owner":"UOTHER","enabled":false})
                .to_string();
        db.set_slack_configuration(&other).await.unwrap();
        db.slack_eyes_unavailable(&request.request_id, "message_not_found")
            .await
            .unwrap();
        db.slack_eyes_confirmed(&request.request_id).await.unwrap();
        let retained = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(retained.request_id, request.request_id);
        assert!(!retained.eyes_confirmed);
        assert_eq!(retained.eyes_failure.as_deref(), Some("message_not_found"));
        db.set_slack_configuration(&other).await.unwrap();
    }
    #[tokio::test]
    async fn account_switch_cannot_orphan_accepted_work() {
        let db = Database::open_in_memory().await.unwrap();
        let source = source("100.000", SlackRequestKind::Briefing);
        let original =
            serde_json::json!({"workspace":source.workspace,"owner":source.owner,"enabled":true})
                .to_string();
        db.set_slack_configuration(&original).await.unwrap();
        db.accept_slack_source(&source, false).await.unwrap();
        db.set_slack_configuration(&original).await.unwrap();
        let other =
            serde_json::json!({"workspace":"other.slack.com","owner":"UOTHER","enabled":false})
                .to_string();
        assert!(db.set_slack_configuration(&other).await.is_err());
        assert_eq!(
            db.get_app_setting("slack_gateway").await.unwrap(),
            Some(original)
        );
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        db.fail_slack_before_admission(
            &request.request_id,
            "fixture failure",
            "https://phoenix.example",
        )
        .await
        .unwrap();
        db.slack_eyes_confirmed(&request.request_id).await.unwrap();
        db.set_slack_configuration(&other).await.unwrap();
        assert_eq!(
            db.slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap()[0]
                .state,
            SlackReturnState::Failed
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Exercise the real atomic drain and both terminal capture paths together.
    async fn drained_batch_completion_covers_every_slack_input_but_not_pending_work() {
        use phoenix_core::domain::{
            db_schema::{InputOrigin, Message, MessageContent},
            sm_event::SteerEntry,
            sm_state::ConvState,
        };
        for (private_first, legacy_checkpoint, private_later) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, false, true),
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let conv = db
                .create_conversation("batch", "batch", "/tmp", true, None, None)
                .await
                .unwrap();
            let mut requests = Vec::new();
            for ts in ["100.000001", "100.000002", "100.000003"] {
                db.accept_slack_source(&source(ts, SlackRequestKind::Explicit), false)
                    .await
                    .unwrap();
            }
            requests.extend(
                db.slack_requests("example.slack.com", "UOWNER")
                    .await
                    .unwrap(),
            );
            db.slack_bind_product(
                &requests[0].binding_id,
                conv.product_conversation_id.as_str(),
            )
            .await
            .unwrap();
            let mut entries = Vec::new();
            let private_entry = SteerEntry {
                text: "private".into(),
                origin: InputOrigin::UnknownHistorical,
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: "private".into(),
                user_agent: None,
                skill_invocation: None,
            };
            if private_first {
                entries.push(private_entry.clone());
            }
            for r in &requests {
                entries.push(SteerEntry {
                    text: r.input_id.clone(),
                    origin: InputOrigin::SlackGateway {
                        request_id: r.request_id.clone(),
                    },
                    llm_text: None,
                    images: vec![],
                    files: vec![],
                    message_id: r.input_id.clone(),
                    user_agent: None,
                    skill_invocation: None,
                });
                db.slack_mark_admitted(&r.request_id, &conv.id)
                    .await
                    .unwrap();
            }
            if private_later {
                entries.insert(entries.len() - 1, private_entry);
            }
            for e in &entries {
                db.append_steering_entry(&conv.id, e, "batch-fixture")
                    .await
                    .unwrap();
            }
            let drain_count = entries.len() - 1 - usize::from(private_later);
            let drained: Vec<_> = entries[..drain_count]
                .iter()
                .enumerate()
                .map(|(n, e)| {
                    let content = MessageContent::user(&e.text);
                    Message {
                        message_id: e.message_id.clone(),
                        origin: e.origin.clone(),
                        conversation_id: conv.id.clone(),
                        sequence_id: i64::try_from(n + 1).unwrap(),
                        message_type: content.message_type(),
                        content,
                        display_data: None,
                        usage_data: None,
                        created_at: chrono::Utc::now(),
                    }
                })
                .collect();
            db.commit_steering_drain(
                &conv.id,
                &drained,
                &ConvState::LlmRequesting { attempt: 1 },
                chrono::Utc::now(),
            )
            .await
            .unwrap();
            let mut sequence = i64::try_from(drained.len() + 1).unwrap();
            if private_later {
                db.add_message_with_seq(
                    "interim",
                    &conv.id,
                    sequence,
                    &MessageContent::agent(vec![]),
                    None,
                    None,
                )
                .await
                .unwrap();
                sequence += 1;
                let e = &entries[drain_count];
                let mut message = drained[0].clone();
                message.message_id.clone_from(&e.message_id);
                message.origin.clone_from(&e.origin);
                message.content = MessageContent::user(&e.text);
                message.sequence_id = sequence;
                db.commit_steering_drain(
                    &conv.id,
                    &[message],
                    &ConvState::LlmRequesting { attempt: 1 },
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
                sequence += 1;
            }
            if legacy_checkpoint {
                db.add_message_with_seq(
                    "answer",
                    &conv.id,
                    sequence,
                    &MessageContent::agent(vec![]),
                    None,
                    None,
                )
                .await
                .unwrap();
                sqlx::query("UPDATE conversations SET state='{\"type\":\"idle\"}',state_kind='idle' WHERE id=?1").bind(&conv.id).execute(db.pool()).await.unwrap();
            } else {
                db.add_message_and_clear_provider_replay(
                    "answer",
                    &conv.id,
                    sequence,
                    &MessageContent::agent(vec![]),
                    None,
                    None,
                    &ConvState::Idle,
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
            }
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            let returns = db
                .slack_returns("example.slack.com", "UOWNER")
                .await
                .unwrap();
            let covered: std::collections::BTreeSet<_> = returns
                .iter()
                .flat_map(|r| r.request_ids.iter().cloned())
                .collect();
            let expected: std::collections::BTreeSet<_> =
                requests[..2].iter().map(|r| r.request_id.clone()).collect();
            assert_eq!(
                covered, expected,
                "private_first={private_first}, legacy={legacy_checkpoint}"
            );
            assert_eq!(returns.len(), 1);
            assert_eq!(db.get_steering_queue(&conv.id).await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn generated_body_requires_exact_owner_and_an_ambiguous_send() {
        let db = Database::open_in_memory().await.unwrap();
        let source = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&source, false).await.unwrap();
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,state,created_at_unix_micros) VALUES ('copy',?1,'copy','thread','same words','uncertain',1)").bind(&request.binding_id).execute(db.pool()).await.unwrap();
        for (author, expected) in [("UOTHER", false), ("UOWNER", true)] {
            assert_eq!(
                db.is_gateway_slack_source(
                    &source.workspace,
                    &source.owner,
                    &source.channel,
                    "200.000",
                    Some((author, "Phoenix: same words"))
                )
                .await
                .unwrap(),
                expected
            );
        }
        assert!(!db
            .is_gateway_slack_source(
                &source.workspace,
                &source.owner,
                &source.channel,
                "200.000",
                None
            )
            .await
            .unwrap());
        sqlx::query("UPDATE slack_gateway_returns SET state='failed' WHERE return_id='copy'")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(!db
            .is_gateway_slack_source(
                &source.workspace,
                &source.owner,
                &source.channel,
                "200.000",
                Some(("UOWNER", "Phoenix: same words"))
            )
            .await
            .unwrap());
        sqlx::query("UPDATE slack_gateway_returns SET state='confirmed',delivery_proof='{\"message_ts\":\"200.000\"}' WHERE return_id='copy'").execute(db.pool()).await.unwrap();
        assert!(db
            .is_gateway_slack_source(
                &source.workspace,
                &source.owner,
                &source.channel,
                "200.000",
                None
            )
            .await
            .unwrap());
    }
    #[tokio::test]
    async fn gateway_timestamps_reject_negative_and_noninteger_values() {
        let db = Database::open_in_memory().await.unwrap();
        let source = source("100.000", SlackRequestKind::Briefing);
        db.slack_baseline_complete(&source.workspace, &source.owner)
            .await
            .unwrap();
        db.accept_slack_source(&source, false).await.unwrap();
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        db.fail_slack_before_admission(
            &request.request_id,
            "fixture refusal",
            "https://phoenix.example",
        )
        .await
        .unwrap();
        for (table, column) in [
            ("slack_gateway_requests", "accepted_at_unix_micros"),
            ("slack_gateway_requests", "observed_at_unix_micros"),
            ("slack_gateway_returns", "created_at_unix_micros"),
            ("slack_gateway_accounts", "last_scan_at_unix_micros"),
            ("slack_gateway_accounts", "retry_at_us"),
        ] {
            for invalid in ["-1", "1.5", "'not a timestamp'"] {
                assert!(
                    sqlx::query(sqlx::AssertSqlSafe(format!(
                        "UPDATE {table} SET {column}={invalid}"
                    )))
                    .execute(db.pool())
                    .await
                    .is_err(),
                    "{table}.{column} accepted {invalid}"
                );
            }
        }
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One retained fixture covers outcomes and interrupted claims across reopen.
    async fn private_notification_attempts_and_eyes_claims_survive_restart_without_retry() {
        for outcome in [
            None,
            Some(SlackReturnState::Submitted),
            Some(SlackReturnState::Failed),
            Some(SlackReturnState::Uncertain),
            Some(SlackReturnState::KnownUnsent),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("once.db");
            let db = Database::open(path.to_str().unwrap()).await.unwrap();
            crate::migrations::run_pending_migrations(db.pool())
                .await
                .unwrap();
            let source = source("100.000", SlackRequestKind::Briefing);
            db.accept_slack_source(&source, false).await.unwrap();
            let request = db
                .slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            assert!(db
                .slack_begin_eyes_attempt(&request.request_id)
                .await
                .unwrap());
            db.fail_slack_before_admission(
                &request.request_id,
                "fixture rejection",
                "https://phoenix.example",
            )
            .await
            .unwrap();
            let output = db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            assert!(db
                .slack_begin_notification_return(
                    &output.return_id,
                    "first",
                    &["/example/notify".into()],
                    "https://phoenix.example/frozen"
                )
                .await
                .unwrap());
            if let Some(outcome) = outcome {
                if outcome == SlackReturnState::KnownUnsent {
                    sqlx::query(
                        "UPDATE slack_gateway_returns SET state='known_unsent' WHERE return_id=?1",
                    )
                    .bind(&output.return_id)
                    .execute(db.pool())
                    .await
                    .unwrap();
                } else {
                    db.slack_finish_return(
                        &output.return_id,
                        "first",
                        outcome,
                        None,
                        (outcome == SlackReturnState::Submitted).then_some("receipt"),
                        None,
                    )
                    .await
                    .unwrap();
                }
            }
            db.pool().close().await;
            let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
            reopened.slack_recover_returns().await.unwrap();
            assert!(!reopened
                .slack_begin_eyes_attempt(&request.request_id)
                .await
                .unwrap());
            assert!(!reopened
                .slack_begin_notification_return(
                    &output.return_id,
                    "retry",
                    &["/replacement/notify".into()],
                    "https://replacement.example"
                )
                .await
                .unwrap());
            let retained = reopened
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            assert_eq!(
                retained.state,
                match outcome {
                    None => SlackReturnState::Uncertain,
                    Some(SlackReturnState::KnownUnsent) => SlackReturnState::Failed,
                    Some(state) => state,
                }
            );
            let command: String = sqlx::query_scalar(
                "SELECT notification_command FROM slack_gateway_returns WHERE return_id=?1",
            )
            .bind(&output.return_id)
            .fetch_one(reopened.pool())
            .await
            .unwrap();
            assert_eq!(command, r#"["/example/notify"]"#);
            reopened
                .set_slack_configuration(
                    r#"{"enabled":false,"workspace":"other.slack.com","owner":"UOTHER"}"#,
                )
                .await
                .expect("best-effort delivery cannot block an otherwise settled account");
        }
    }

    #[tokio::test]
    async fn legacy_question_text_is_replaced_only_before_any_possible_delivery() {
        for state in [
            SlackReturnState::Pending,
            SlackReturnState::KnownUnsent,
            SlackReturnState::Attempting,
            SlackReturnState::Uncertain,
            SlackReturnState::Confirmed,
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let source = source("100.000", SlackRequestKind::Explicit);
            db.accept_slack_source(&source, false).await.unwrap();
            let request = db
                .slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,response_kind,state,delivery_proof,created_at_unix_micros) VALUES ('legacy',?1,'legacy','thread','Confidential question and options','clarification',?2,?3,1)")
                .bind(&request.binding_id).bind(state.as_str()).bind((state==SlackReturnState::Confirmed).then_some("proof")).execute(db.pool()).await.unwrap();
            sqlx::query("INSERT INTO slack_gateway_return_coverage(return_id,request_id) VALUES ('legacy',?1)").bind(&request.request_id).execute(db.pool()).await.unwrap();
            sqlx::query("INSERT INTO slack_gateway_questions(return_id,question_id,transcript_id) VALUES ('legacy','question','gone')").execute(db.pool()).await.unwrap();
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            let outputs = db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap();
            let old = outputs.iter().find(|r| r.return_id == "legacy").unwrap();
            assert_eq!(old.text, "Confidential question and options");
            if matches!(
                state,
                SlackReturnState::Pending | SlackReturnState::KnownUnsent
            ) {
                assert_eq!(outputs.len(), 2);
                assert_eq!(old.state, SlackReturnState::Failed);
                let handoff = outputs.iter().find(|r| r.return_id != "legacy").unwrap();
                assert!(handoff.text.starts_with(CLARIFICATION_HANDOFF));
                assert!(!handoff.text.contains("Confidential"));
                assert_eq!(handoff.request_ids, vec![request.request_id]);
                assert!(db
                    .slack_begin_return(&handoff.return_id, "new")
                    .await
                    .unwrap());
            } else {
                assert_eq!(outputs.len(), 1);
                assert_eq!(old.state, state);
            }
            db.process_slack_native_outcomes("https://changed.example")
                .await
                .unwrap();
            assert_eq!(
                db.slack_returns(&source.workspace, &source.owner)
                    .await
                    .unwrap()
                    .len(),
                outputs.len()
            );
        }
    }

    fn source(ts: &str, kind: SlackRequestKind) -> SlackSource {
        SlackSource {
            workspace: "example.slack.com".into(),
            owner: "UOWNER".into(),
            channel: "CCHANNEL".into(),
            message_ts: ts.into(),
            thread_ts: "100.000".into(),
            source_url: format!(
                "https://example.slack.com/archives/CCHANNEL/p{}",
                ts.replace('.', "")
            ),
            instruction: "investigate".into(),
            context: "quoted source".into(),
            observed_at_unix_micros: chrono::Utc::now().timestamp_micros(),
            trigger_kind: if kind == SlackRequestKind::Explicit {
                SlackTriggerKind::Prefix
            } else {
                SlackTriggerKind::Reaction
            },
            kind,
        }
    }
    #[tokio::test]
    async fn source_identity_retains_first_verified_trigger_provenance() {
        let db = Database::open_in_memory().await.unwrap();
        let mut source = source("100.000001", SlackRequestKind::Explicit);
        source.trigger_kind = SlackTriggerKind::Reaction;
        db.accept_slack_source(&source, false).await.unwrap();
        source.trigger_kind = SlackTriggerKind::Prefix;
        db.accept_slack_source(&source, false).await.unwrap();
        let stored = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].source.trigger_kind, SlackTriggerKind::Reaction);
        source.message_ts = "100.000002".into();
        source.trigger_kind = SlackTriggerKind::UnknownHistorical;
        assert!(db.accept_slack_source(&source, false).await.is_err());
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Keep terminal checkpoint and recovery assertions together for each fixture.
    async fn final_text_briefing_records_completion_and_recovers_retained_occurrence_once() {
        use phoenix_core::domain::{
            db_schema::{InputOrigin, MessageContent},
            sm_state::ConvState,
        };
        for (legacy_checkpoint, final_state) in [
            (false, ConvState::Idle),
            (false, ConvState::Terminal),
            (
                false,
                ConvState::Completed {
                    result: "ready".into(),
                },
            ),
            (true, ConvState::Idle),
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let conv = db
                .create_conversation("briefing", "briefing", "/tmp", true, None, None)
                .await
                .unwrap();
            let source = source("100.000", SlackRequestKind::Briefing);
            db.accept_slack_source(&source, false).await.unwrap();
            let request = db
                .slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            db.slack_bind_product(&request.binding_id, conv.product_conversation_id.as_str())
                .await
                .unwrap();
            db.add_message_with_seq_and_origin(
                &request.input_id,
                &conv.id,
                1,
                &MessageContent::user("Prepare a briefing"),
                None,
                None,
                &InputOrigin::SlackGateway {
                    request_id: request.request_id.clone(),
                },
            )
            .await
            .unwrap();
            db.update_conversation_state(&conv.id, &ConvState::LlmRequesting { attempt: 1 })
                .await
                .unwrap();
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            assert!(db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap()
                .is_empty());
            if legacy_checkpoint {
                db.add_message_with_seq(
                    "answer",
                    &conv.id,
                    2,
                    &MessageContent::agent(vec![]),
                    None,
                    None,
                )
                .await
                .unwrap();
                sqlx::query("UPDATE conversations SET state='{\"type\":\"idle\"}',state_kind='idle' WHERE id=?1")
                    .bind(&conv.id).execute(db.pool()).await.unwrap();
            } else {
                db.add_message_and_clear_provider_replay(
                    "answer",
                    &conv.id,
                    2,
                    &MessageContent::agent(vec![]),
                    None,
                    None,
                    &final_state,
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
                let captured: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM slack_gateway_native_outcomes WHERE request_id=?1",
                )
                .bind(&request.request_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
                assert_eq!(captured, 1);
            }
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            let outputs = db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap();
            assert_eq!(outputs.len(), 1);
            assert_eq!(outputs[0].destination, "private");
            assert_eq!(outputs[0].text, "Phoenix: your briefing is ready");
            assert_eq!(outputs[0].request_ids, vec![request.request_id]);
            let retained: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM steering_execution_occurrences WHERE conversation_id=?1",
            )
            .bind(&conv.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(retained, 0);
        }
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Compare private input isolation with an actual request's continuation lifecycle.
    async fn native_private_input_does_not_publish_and_automatic_continuation_defers_failure() {
        use phoenix_core::domain::{
            db_schema::{InputOrigin, MessageContent},
            product_conversation::AutoContinueOnContextExhaustion,
            sm_state::ConvState,
        };
        let db = Database::open_in_memory().await.unwrap();
        let conv = db
            .create_conversation("native", "native", "/tmp", true, None, None)
            .await
            .unwrap();
        let source = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&source, false).await.unwrap();
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        db.slack_bind_product(&request.binding_id, conv.product_conversation_id.as_str())
            .await
            .unwrap();
        db.add_message_with_seq_and_origin(
            &request.input_id,
            &conv.id,
            1,
            &MessageContent::user("ask"),
            None,
            None,
            &InputOrigin::SlackGateway {
                request_id: request.request_id,
            },
        )
        .await
        .unwrap();
        // The original direct workflow may still own the conversation while
        // a private steering occurrence has already become the active input.
        sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'fixture',1,1,1,0,0,'Active','fixture',1,X'00',1,1)").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,canonical_message_id,materialized) VALUES (1,1,?1,'fixture','fixture',X'00','Runtime',0,1,?2,1)").bind(&conv.id).bind(&request.input_id).execute(db.pool()).await.unwrap();
        db.add_message_with_seq_and_origin(
            "private-input",
            &conv.id,
            2,
            &MessageContent::user("private follow-up"),
            None,
            None,
            &InputOrigin::UserApi,
        )
        .await
        .unwrap();
        db.update_conversation_state(
            &conv.id,
            &ConvState::AwaitingUserResponse {
                questions: vec![phoenix_core::domain::sm_state::UserQuestion {
                    header: "Private".into(),
                    question: "Private clarification?".into(),
                    options: vec![],
                    multi_select: false,
                }],
                tool_use_id: "private-question".into(),
                request_authority: phoenix_core::domain::sm_state::QuestionRequestAuthority::new(),
            },
        )
        .await
        .unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM slack_gateway_native_outcomes WHERE kind='question'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            0
        );
        assert!(db
            .slack_returns("example.slack.com", "UOWNER")
            .await
            .unwrap()
            .is_empty());
        sqlx::query("DELETE FROM durable_turns WHERE turn_id=1")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM workflows WHERE workflow_id=1")
            .execute(db.pool())
            .await
            .unwrap();
        let mut tx = db.pool().begin().await.unwrap();
        record_slack_native_outcome_tx(
            &mut tx,
            &conv.id,
            "private-input",
            "private-complete",
            "Completed",
            false,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        assert!(db
            .slack_returns(&source.workspace, &source.owner)
            .await
            .unwrap()
            .is_empty());
        db.set_auto_continue_on_context_exhaustion(
            &conv.product_conversation_id,
            AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .unwrap();
        db.update_conversation_state(
            &conv.id,
            &ConvState::ContextExhausted {
                summary: "summary".into(),
            },
        )
        .await
        .unwrap();
        let mut tx = db.pool().begin().await.unwrap();
        record_slack_native_outcome_tx(
            &mut tx,
            &conv.id,
            &request.input_id,
            "request-exhausted",
            "Failed",
            true,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        assert!(db
            .slack_returns(&source.workspace, &source.owner)
            .await
            .unwrap()
            .is_empty());
        db.set_auto_continue_on_context_exhaustion(
            &conv.product_conversation_id,
            AutoContinueOnContextExhaustion::Disabled,
        )
        .await
        .unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        let outputs = db
            .slack_returns(&source.workspace, &source.owner)
            .await
            .unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].destination, "thread");
    }

    // REQ-SG-012: Slack ordering never answers a native clarification.
    #[tokio::test]
    async fn later_slack_requests_never_infer_a_question_answer() {
        for count in [1, 2] {
            let db = Database::open_in_memory().await.unwrap();
            let conv = db
                .create_conversation("questions", "questions", "/tmp", true, None, None)
                .await
                .unwrap();
            let original = source("100.000", SlackRequestKind::Explicit);
            db.accept_slack_source(&original, false).await.unwrap();
            let first = db
                .slack_requests(&original.workspace, &original.owner)
                .await
                .unwrap()
                .remove(0);
            db.slack_bind_product(&first.binding_id, conv.product_conversation_id.as_str())
                .await
                .unwrap();
            for n in 0..count {
                let id = format!("q{n}");
                sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,response_kind,state,delivery_proof,created_at_unix_micros) VALUES (?1,?2,?1,'thread','Question','clarification','confirmed','{\"message_ts\":\"101.000\"}',1)").bind(&id).bind(&first.binding_id).execute(db.pool()).await.unwrap();
                sqlx::query("INSERT INTO slack_gateway_questions(return_id,question_id,transcript_id) VALUES (?1,?1,?2)").bind(&id).bind(&conv.id).execute(db.pool()).await.unwrap();
            }
            let mut next = source("102.000", SlackRequestKind::Explicit);
            next.instruction = "deploy staging".into();
            db.accept_slack_source(&next, false).await.unwrap();
            let accepted = db
                .slack_requests(&original.workspace, &original.owner)
                .await
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(accepted.source.instruction, "deploy staging");
            assert_eq!(accepted.binding_id, first.binding_id);
            assert!(accepted.question_id.is_none());
            assert!(accepted.question_transcript.is_none());
            assert!(accepted.failure.is_none());
            assert!(!accepted.admitted);
        }
    }

    // REQ-SG-012/013: the handoff ends automatic publication for this request.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn clarification_handoff_freezes_link_and_does_not_inherit_private_discussion() {
        use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
        let db = Database::open_in_memory().await.unwrap();
        let conv = db
            .create_conversation("handoff", "handoff", "/tmp", true, None, None)
            .await
            .unwrap();
        let original = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&original, false).await.unwrap();
        let request = db
            .slack_requests(&original.workspace, &original.owner)
            .await
            .unwrap()
            .remove(0);
        db.slack_bind_product(&request.binding_id, conv.product_conversation_id.as_str())
            .await
            .unwrap();
        db.slack_mark_admitted(&request.request_id, &conv.id)
            .await
            .unwrap();
        db.slack_eyes_confirmed(&request.request_id).await.unwrap();
        let authority = phoenix_core::domain::sm_state::QuestionRequestAuthority::new();
        db.update_conversation_state(
            &conv.id,
            &phoenix_core::domain::sm_state::ConvState::AwaitingUserResponse {
                questions: vec![],
                tool_use_id: "question-tool".into(),
                request_authority: authority.clone(),
            },
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id,question_id,question_body) VALUES (?1,'question','question',?2,?3,'[{\"header\":\"Repo\",\"question\":\"Which repository?\",\"options\":[],\"multi_select\":false}]')")
            .bind(&request.request_id).bind(&conv.id).bind(authority.request_id().unwrap().to_string()).execute(db.pool()).await.unwrap();
        db.process_slack_native_outcomes("https://phoenix.example/")
            .await
            .unwrap();
        let output = db
            .slack_returns(&original.workspace, &original.owner)
            .await
            .unwrap()
            .remove(0);
        assert!(!output.text.contains("Which repository?"));
        assert!(output.text.starts_with(CLARIFICATION_HANDOFF));
        assert!(output.text.ends_with(&format!(
            "https://phoenix.example/c/{}",
            conv.product_conversation_id
        )));
        assert_eq!(output.request_ids, vec![request.request_id.clone()]);
        assert!(db
            .slack_begin_return(&output.return_id, "first")
            .await
            .unwrap());
        db.slack_finish_return(
            &output.return_id,
            "first",
            SlackReturnState::KnownUnsent,
            None,
            None,
            Some("not dispatched"),
        )
        .await
        .unwrap();
        db.process_slack_native_outcomes("https://changed.example")
            .await
            .unwrap();
        assert_eq!(
            db.slack_returns(&original.workspace, &original.owner)
                .await
                .unwrap()[0]
                .text,
            output.text
        );
        assert!(db
            .slack_begin_return(&output.return_id, "retry")
            .await
            .unwrap());
        db.slack_finish_return(
            &output.return_id,
            "retry",
            SlackReturnState::Confirmed,
            Some("proof"),
            None,
            None,
        )
        .await
        .unwrap();
        db.add_message_with_seq_and_origin(
            "private-answer",
            &conv.id,
            1,
            &MessageContent::user("Use private-repo"),
            None,
            None,
            &InputOrigin::UserApi,
        )
        .await
        .unwrap();
        let mut tx = db.pool().begin().await.unwrap();
        assert!(
            requests_for_native_input(&mut tx, &conv.id, "private-answer")
                .await
                .unwrap()
                .is_empty()
        );
        record_slack_native_outcome_tx(
            &mut tx,
            &conv.id,
            "private-answer",
            "private-complete",
            "Completed",
            false,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        db.process_slack_native_outcomes("https://changed.example")
            .await
            .unwrap();
        assert_eq!(
            db.slack_returns(&original.workspace, &original.owner)
                .await
                .unwrap()
                .len(),
            1
        );
        db.set_slack_configuration(
            r#"{"enabled":false,"workspace":"other.slack.com","owner":"UOTHER"}"#,
        )
        .await
        .expect("confirmed handoff must not strand account switching");
    }

    // REQ-SG-012/013: a private answer before materialization cannot erase the duty.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn retired_questions_preserve_generic_handoffs_before_and_after_materialization() {
        use phoenix_core::domain::sm_state::{ConvState, QuestionRequestAuthority};
        for (replaced, preexisting) in [false, true].into_iter().flat_map(|replaced| {
            [
                None,
                Some(SlackReturnState::Pending),
                Some(SlackReturnState::KnownUnsent),
                Some(SlackReturnState::Uncertain),
            ]
            .map(move |state| (replaced, state))
        }) {
            let db = Database::open_in_memory().await.unwrap();
            let conv = db
                .create_conversation("early", "early", "/tmp", true, None, None)
                .await
                .unwrap();
            let source = source("100.000", SlackRequestKind::Explicit);
            db.accept_slack_source(&source, false).await.unwrap();
            let request = db
                .slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            db.slack_bind_product(&request.binding_id, conv.product_conversation_id.as_str())
                .await
                .unwrap();
            db.slack_mark_admitted(&request.request_id, &conv.id)
                .await
                .unwrap();
            let original = QuestionRequestAuthority::new();
            db.update_conversation_state(
                &conv.id,
                &ConvState::AwaitingUserResponse {
                    questions: vec![],
                    tool_use_id: "original".into(),
                    request_authority: original.clone(),
                },
            )
            .await
            .unwrap();
            sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id,question_id,question_body) VALUES (?1,'original','question',?2,?3,'[{\"header\":\"Old\",\"question\":\"Stale confidential question\",\"options\":[],\"multi_select\":false}]')")
                .bind(&request.request_id).bind(&conv.id).bind(original.request_id().unwrap().to_string()).execute(db.pool()).await.unwrap();
            let old = if let Some(state) = preexisting {
                db.process_slack_native_outcomes("https://phoenix.example")
                    .await
                    .unwrap();
                let old = db
                    .slack_returns(&source.workspace, &source.owner)
                    .await
                    .unwrap()
                    .remove(0);
                if state != SlackReturnState::Pending {
                    assert!(db
                        .slack_begin_return(&old.return_id, "attempt")
                        .await
                        .unwrap());
                    db.slack_finish_return(
                        &old.return_id,
                        "attempt",
                        state,
                        None,
                        None,
                        Some("fixture result"),
                    )
                    .await
                    .unwrap();
                }
                Some(old)
            } else {
                None
            };
            let state = if replaced {
                ConvState::AwaitingUserResponse {
                    questions: vec![],
                    tool_use_id: "private-question".into(),
                    request_authority: QuestionRequestAuthority::new(),
                }
            } else {
                ConvState::Idle
            };
            db.update_conversation_state(&conv.id, &state)
                .await
                .unwrap();
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            let outputs = db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap();
            if preexisting == Some(SlackReturnState::Uncertain) {
                assert_eq!(outputs.len(), 1);
                assert_eq!(outputs[0].state, SlackReturnState::Uncertain);
                continue;
            }
            assert_eq!(outputs.len(), 1);
            let generic = &outputs[0];
            if let Some(old) = &old {
                assert_eq!(generic.return_id, old.return_id);
                assert_eq!(generic.text, old.text);
            }
            assert!(generic.text.starts_with(CLARIFICATION_HANDOFF));
            assert!(!generic.text.contains("Stale confidential question"));
            assert!(generic
                .text
                .ends_with(&format!("/c/{}", conv.product_conversation_id)));
            assert_eq!(generic.request_ids, vec![request.request_id]);
            db.update_conversation_state(&conv.id, &ConvState::Idle)
                .await
                .unwrap();
            db.process_slack_native_outcomes("https://changed.example")
                .await
                .unwrap();
            let repeated = db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap();
            assert_eq!(repeated.len(), outputs.len());
            let retry = repeated
                .iter()
                .find(|r| r.return_id == generic.return_id)
                .unwrap();
            assert_eq!(retry.state, generic.state);
            assert_eq!(retry.text, generic.text);
        }
    }

    #[tokio::test]
    async fn candidate_observation_transfers_atomically_without_granting_authority() {
        let db = Database::open_in_memory().await.unwrap();
        let mut source = source("100.000", SlackRequestKind::Explicit);
        for at in [100, 200] {
            db.slack_observe_source(
                &source.workspace,
                &source.owner,
                &source.channel,
                &source.message_ts,
                at,
            )
            .await
            .unwrap();
        }
        db.slack_observe_source(
            &source.workspace,
            "UOTHER",
            &source.channel,
            &source.message_ts,
            400,
        )
        .await
        .unwrap();
        assert!(!db
            .slack_source_recorded(
                &source.workspace,
                &source.owner,
                &source.channel,
                &source.message_ts
            )
            .await
            .unwrap());
        assert!(db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .is_empty());
        source.observed_at_unix_micros = 300;
        db.accept_slack_source(&source, false).await.unwrap();
        let accepted = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(accepted.source.observed_at_unix_micros, 100);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT owner FROM slack_gateway_observations")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            "UOTHER"
        );
        assert!(db
            .slack_observe_source(
                &source.workspace,
                &source.owner,
                &source.channel,
                "200.000",
                -1
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn settled_continuation_retires_exhaustion_processing_without_a_failure_reply() {
        use phoenix_core::domain::{
            db_schema::MessageContent, product_conversation::AutoContinueOnContextExhaustion,
            sm_state::ConvState,
        };
        let db = Database::open_in_memory().await.unwrap();
        let conv = db
            .create_conversation("settled", "settled", "/tmp", true, None, None)
            .await
            .unwrap();
        let source = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&source, false).await.unwrap();
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        db.set_auto_continue_on_context_exhaustion(
            &conv.product_conversation_id,
            AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .unwrap();
        db.update_conversation_state(
            &conv.id,
            &ConvState::ContextExhausted {
                summary: "summary".into(),
            },
        )
        .await
        .unwrap();
        let summary = db
            .add_message(
                "summary",
                &conv.id,
                &MessageContent::continuation("summary"),
                None,
                None,
            )
            .await
            .unwrap();
        sqlx::query("INSERT INTO automatic_continuation_admissions(predecessor_conversation_id,product_conversation_id,summary_message_id,operation_id,first_message_id,phase,admitted_at_unix_micros,updated_at_unix_micros) VALUES (?1,?2,?3,'operation','successor-input','message_settled',1,1)")
            .bind(&conv.id).bind(conv.product_conversation_id.as_str()).bind(&summary.message_id).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id,context_exhausted) VALUES (?1,'exhausted','Failed',?2,1)")
            .bind(&request.request_id).bind(&conv.id).execute(db.pool()).await.unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        assert!(db
            .slack_returns(&source.workspace, &source.owner)
            .await
            .unwrap()
            .is_empty());
        let retained: (bool, String) = sqlx::query_as("SELECT processed,request_id FROM slack_gateway_native_outcomes WHERE occurrence_key='exhausted'").fetch_one(db.pool()).await.unwrap();
        assert_eq!(retained, (true, request.request_id));
    }

    #[tokio::test]
    async fn deleted_exhaustion_source_does_not_block_other_native_outcomes() {
        let db = Database::open_in_memory().await.unwrap();
        let conv = db
            .create_conversation("deleted", "deleted", "/tmp", true, None, None)
            .await
            .unwrap();
        let old = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&old, false).await.unwrap();
        let first = db
            .slack_requests(&old.workspace, &old.owner)
            .await
            .unwrap()
            .remove(0);
        db.slack_bind_product(&first.binding_id, conv.product_conversation_id.as_str())
            .await
            .unwrap();
        sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id,context_exhausted) VALUES (?1,'exhausted','Failed',?2,1)")
            .bind(&first.request_id).bind(&conv.id).execute(db.pool()).await.unwrap();
        sqlx::query("DELETE FROM conversations WHERE id=?1")
            .bind(&conv.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM product_conversations WHERE id=?1")
            .bind(conv.product_conversation_id.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        let fresh = source("200.000", SlackRequestKind::Briefing);
        db.accept_slack_source(&fresh, false).await.unwrap();
        let second = db
            .slack_requests(&old.workspace, &old.owner)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.source.kind == SlackRequestKind::Briefing)
            .unwrap();
        sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id) VALUES (?1,'ready','Completed','another-transcript')")
            .bind(&second.request_id).execute(db.pool()).await.unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        let outputs = db.slack_returns(&old.workspace, &old.owner).await.unwrap();
        assert_eq!(outputs.len(), 2);
        let failure = outputs
            .iter()
            .find(|r| r.request_ids == vec![first.request_id.clone()])
            .unwrap();
        assert!(failure.text.ends_with(&format!(
            "/settings/slack#slack-return-{}",
            failure.return_id
        )));
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        assert_eq!(
            db.slack_returns(&old.workspace, &old.owner)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn briefing_notices_are_deduplicated_by_occurrence() {
        use phoenix_core::domain::sm_state::{ConvState, QuestionRequestAuthority};
        for terminal in ["Completed", "Failed", "Deleted"] {
            let db = Database::open_in_memory().await.unwrap();
            let conv = db
                .create_conversation("brief", "brief", "/tmp", true, None, None)
                .await
                .unwrap();
            let source = source("100.000", SlackRequestKind::Briefing);
            db.set_slack_configuration(&serde_json::json!({"workspace":source.workspace,"owner":source.owner,"enabled":true}).to_string()).await.unwrap();
            db.accept_slack_source(&source, false).await.unwrap();
            let request = db
                .slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            let authority = QuestionRequestAuthority::new();
            db.slack_bind_product(&request.binding_id, conv.product_conversation_id.as_str())
                .await
                .unwrap();
            db.slack_mark_admitted(&request.request_id, &conv.id)
                .await
                .unwrap();
            db.slack_eyes_confirmed(&request.request_id).await.unwrap();
            db.update_conversation_state(
                &conv.id,
                &ConvState::AwaitingUserResponse {
                    questions: vec![],
                    tool_use_id: "q".into(),
                    request_authority: authority.clone(),
                },
            )
            .await
            .unwrap();
            sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id,question_id,question_body) VALUES (?1,'question','question',?2,?3,'[]')")
                .bind(&request.request_id).bind(&conv.id).bind(authority.request_id().unwrap().to_string()).execute(db.pool()).await.unwrap();
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            let notice = db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            assert!(notice.text.contains("needs your input"));
            sqlx::query("UPDATE slack_gateway_returns SET state='confirmed',delivery_proof='fixture' WHERE return_id=?1").bind(&notice.return_id).execute(db.pool()).await.unwrap();
            let other_owner =
                serde_json::json!({"workspace":source.workspace,"owner":"UOTHER","enabled":false})
                    .to_string();
            let other_workspace = serde_json::json!({"workspace":"other.slack.com","owner":source.owner,"enabled":false}).to_string();
            for changed in [&other_owner, &other_workspace] {
                assert!(db.set_slack_configuration(changed).await.is_err());
            }
            if terminal == "Deleted" {
                sqlx::raw_sql("CREATE TRIGGER fail_deleted_outcome BEFORE INSERT ON slack_gateway_native_outcomes WHEN NEW.occurrence_key LIKE 'deleted:%' BEGIN SELECT RAISE(ABORT,'fixture deletion failure'); END;")
                    .execute(db.pool()).await.unwrap();
                assert!(db.delete_conversation(&conv.id).await.is_err());
                assert!(db.get_conversation(&conv.id).await.is_ok());
                sqlx::raw_sql("DROP TRIGGER fail_deleted_outcome")
                    .execute(db.pool())
                    .await
                    .unwrap();
                db.delete_conversation(&conv.id).await.unwrap();
            } else {
                db.update_conversation_state(&conv.id, &ConvState::Idle)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO slack_gateway_native_outcomes(request_id,occurrence_key,kind,transcript_id) VALUES (?1,'terminal',?2,?3)")
                    .bind(&request.request_id).bind(terminal).bind(&conv.id).execute(db.pool()).await.unwrap();
            }
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            let outputs = db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap();
            assert_eq!(outputs.len(), 2);
            db.set_slack_configuration(&other_owner).await.expect(
                "terminal native work permits switching without waiting for a private alert",
            );
            assert!(outputs.iter().all(|r| r.destination == "private"));
            assert!(outputs.iter().any(|r| r.text.contains(match terminal {
                "Completed" => "ready",
                "Deleted" => "conversation was deleted",
                _ => "stopped",
            })));
            if terminal != "Deleted" {
                db.delete_conversation(&conv.id).await.unwrap();
            }
            db.process_slack_native_outcomes("https://phoenix.example")
                .await
                .unwrap();
            assert_eq!(
                db.slack_returns(&source.workspace, &source.owner)
                    .await
                    .unwrap()
                    .len(),
                2
            );
        }
    }

    #[tokio::test]
    async fn deletion_preserves_a_completed_briefing_checkpoint_and_rolls_back_capture_failure() {
        use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("checkpoint", "checkpoint", "/tmp", true, None, None)
            .await
            .unwrap();
        let source = source("100.000", SlackRequestKind::Briefing);
        db.accept_slack_source(&source, false).await.unwrap();
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        db.slack_bind_product(
            &request.binding_id,
            conversation.product_conversation_id.as_str(),
        )
        .await
        .unwrap();
        db.slack_mark_admitted(&request.request_id, &conversation.id)
            .await
            .unwrap();
        db.add_message_with_seq_and_origin(
            "checkpoint-input",
            &conversation.id,
            1,
            &MessageContent::user("briefing"),
            None,
            None,
            &InputOrigin::SlackGateway {
                request_id: request.request_id.clone(),
            },
        )
        .await
        .unwrap();
        db.add_message_with_seq(
            "checkpoint-answer",
            &conversation.id,
            2,
            &MessageContent::agent(vec![]),
            None,
            None,
        )
        .await
        .unwrap();
        let checkpoint: String = sqlx::query_scalar(
            "SELECT message_id FROM steering_execution_occurrences WHERE conversation_id=?1",
        )
        .bind(&conversation.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(checkpoint, "checkpoint-input");
        sqlx::raw_sql("CREATE TRIGGER fail_completed_capture BEFORE INSERT ON slack_gateway_native_outcomes WHEN NEW.kind='Completed' BEGIN SELECT RAISE(ABORT,'fixture capture failure'); END;").execute(db.pool()).await.unwrap();
        assert!(db.delete_conversation(&conversation.id).await.is_err());
        assert!(db.get_conversation(&conversation.id).await.is_ok());
        let retained: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM steering_execution_occurrences WHERE conversation_id=?1)",
        )
        .bind(&conversation.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(retained);
        sqlx::raw_sql("DROP TRIGGER fail_completed_capture")
            .execute(db.pool())
            .await
            .unwrap();
        db.delete_conversation(&conversation.id).await.unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM slack_gateway_native_outcomes WHERE request_id=?1",
        )
        .bind(&request.request_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(kinds, vec!["Completed"]);
        let outputs = db
            .slack_returns(&source.workspace, &source.owner)
            .await
            .unwrap();
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].text.contains("briefing is ready"));
        assert!(!outputs[0].published);
        assert!(db.get_conversation(&conversation.id).await.is_err());
    }

    #[tokio::test]
    async fn deleting_a_transcript_preserves_a_live_successor_before_settling_last_member() {
        let db = Database::open_in_memory().await.unwrap();
        let first = db
            .create_conversation("first", "first", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &first.id,
            &super::super::ConvState::ContextExhausted {
                summary: "fixture".into(),
            },
        )
        .await
        .unwrap();
        let super::super::ContinueOutcome::Created(successor) =
            db.continue_conversation(&first.id).await.unwrap()
        else {
            panic!("expected native successor");
        };
        let source = source("100.000", SlackRequestKind::Briefing);
        db.accept_slack_source(&source, false).await.unwrap();
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        db.slack_bind_product(&request.binding_id, first.product_conversation_id.as_str())
            .await
            .unwrap();
        db.slack_mark_admitted(&request.request_id, &first.id)
            .await
            .unwrap();
        db.delete_conversation(&first.id).await.unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        assert!(db
            .slack_returns(&source.workspace, &source.owner)
            .await
            .unwrap()
            .is_empty());
        db.delete_conversation(&successor.id).await.unwrap();
        db.process_slack_native_outcomes("https://phoenix.example")
            .await
            .unwrap();
        let outputs = db
            .slack_returns(&source.workspace, &source.owner)
            .await
            .unwrap();
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].text.contains("conversation was deleted"));
    }

    // REQ-SG-012: a rejected creation has no conversation to link yet.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One lifecycle fixture compares status and generated links with native routing.
    async fn request_status_links_require_native_publication() {
        let db = Database::open_in_memory().await.unwrap();
        let source = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&source, false).await.unwrap();
        let request = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        db.slack_bind_product(&request.binding_id, "reserved-product")
            .await
            .unwrap();
        let pending = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(pending.product_id.as_deref(), Some("reserved-product"));
        assert!(!pending.conversation_available);
        db.fail_slack_before_admission(
            &request.request_id,
            "creation failed",
            "https://phoenix.example",
        )
        .await
        .unwrap();
        assert!(
            !db.slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()[0]
                .conversation_available
        );
        let product = db
            .create_conversation("linked", "linked", "/tmp", true, None, None)
            .await
            .unwrap();
        sqlx::query("UPDATE slack_gateway_bindings SET product_id=?2 WHERE binding_id=?1")
            .bind(&request.binding_id)
            .bind(product.product_conversation_id.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        assert!(
            db.slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()[0]
                .conversation_available
        );
        sqlx::query("INSERT INTO product_creation_jobs(request_id, product_conversation_id, cwd, objective, llm_language, status, accepted_at_unix_micros, updated_at_unix_micros, published_product_id, published_conversation_id, provisioning_stage, stage_updated_at_unix_micros, objective_origin) VALUES (?1,?2,'/tmp','','phoenix-native','delivery_pending',1,1,?2,?3,'bootstrap_initial_turn',1,'user_api')")
            .bind(&request.creation_id).bind(product.product_conversation_id.as_str()).bind(&product.id).execute(db.pool()).await.unwrap();
        for (status, available) in [
            ("delivery_pending", false),
            ("delivery_failed", false),
            ("published", true),
        ] {
            sqlx::query("UPDATE product_creation_jobs SET status=?1 WHERE request_id=?2")
                .bind(status)
                .bind(&request.creation_id)
                .execute(db.pool())
                .await
                .unwrap();
            assert_eq!(
                db.resolve_ordinary_product_conversation(product.product_conversation_id.as_str())
                    .await
                    .is_ok(),
                available
            );
            assert_eq!(
                db.slack_requests(&source.workspace, &source.owner)
                    .await
                    .unwrap()[0]
                    .conversation_available,
                available
            );
            assert_eq!(
                db.slack_returns(&source.workspace, &source.owner)
                    .await
                    .unwrap()[0]
                    .published,
                available
            );
            let mut tx = db.pool().begin().await.unwrap();
            let text = slack_thread_response_tx(
                &mut tx,
                &request.binding_id,
                "test-return",
                "test",
                "https://phoenix.example",
            )
            .await
            .unwrap();
            assert_eq!(text.contains("/c/"), available);
            tx.rollback().await.unwrap();
        }
        db.update_conversation_state(
            &product.id,
            &super::super::ConvState::ContextExhausted {
                summary: "fixture".into(),
            },
        )
        .await
        .unwrap();
        let super::super::ContinueOutcome::Created(successor) =
            db.continue_conversation(&product.id).await.unwrap()
        else {
            panic!("expected native successor");
        };
        db.delete_conversation(&product.id).await.unwrap();
        let creation_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM product_creation_jobs WHERE request_id=?1)",
        )
        .bind(&request.creation_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(!creation_exists);
        assert!(db
            .resolve_ordinary_product_conversation(product.product_conversation_id.as_str())
            .await
            .is_ok());
        assert!(
            db.slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()[0]
                .conversation_available
        );
        assert!(
            db.slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap()[0]
                .published
        );
        db.delete_conversation(&successor.id).await.unwrap();
        let deleted = db
            .slack_requests(&source.workspace, &source.owner)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(
            deleted.product_id,
            Some(product.product_conversation_id.to_string())
        );
        assert!(!deleted.conversation_available);
    }

    #[tokio::test]
    async fn admission_failure_links_to_its_exact_return_status() {
        for unavailable_product in [false, true] {
            let db = Database::open_in_memory().await.unwrap();
            let source = source("100.000", SlackRequestKind::Explicit);
            db.accept_slack_source(&source, false).await.unwrap();
            let request = db
                .slack_requests(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            if unavailable_product {
                db.slack_bind_product(&request.binding_id, "deleted-product")
                    .await
                    .unwrap();
            }
            db.fail_slack_before_admission(
                &request.request_id,
                "creation unavailable",
                "https://phoenix.example/",
            )
            .await
            .unwrap();
            let output = db
                .slack_returns(&source.workspace, &source.owner)
                .await
                .unwrap()
                .remove(0);
            assert!(output.text.ends_with(&format!(
                "https://phoenix.example/settings/slack#slack-return-{}",
                output.return_id
            )));
            assert_eq!(output.request_ids, vec![request.request_id]);
        }
    }

    #[tokio::test]
    async fn acceptance_distinguishes_inactive_gateway_from_recorded_source() {
        let db = Database::open_in_memory().await.unwrap();
        let first = source("100.000", SlackRequestKind::Explicit);
        let mut setting =
            serde_json::json!({"enabled":true,"workspace":first.workspace,"owner":first.owner});
        db.set_app_setting("slack_gateway", &setting.to_string())
            .await
            .unwrap();
        assert_eq!(
            db.accept_slack_source(&first, false).await.unwrap(),
            SlackSourceAcceptance::Recorded
        );
        assert_eq!(
            db.accept_slack_source(&first, false).await.unwrap(),
            SlackSourceAcceptance::Recorded
        );
        setting["enabled"] = serde_json::json!(false);
        db.set_app_setting("slack_gateway", &setting.to_string())
            .await
            .unwrap();
        let next = source("101.000", SlackRequestKind::Explicit);
        assert_eq!(
            db.accept_slack_source(&next, false).await.unwrap(),
            SlackSourceAcceptance::Inactive
        );
        assert!(!db
            .slack_source_recorded(
                &next.workspace,
                &next.owner,
                &next.channel,
                &next.message_ts
            )
            .await
            .unwrap());
        setting["enabled"] = serde_json::json!(true);
        db.set_app_setting("slack_gateway", &setting.to_string())
            .await
            .unwrap();
        assert_eq!(
            db.accept_slack_source(&next, false).await.unwrap(),
            SlackSourceAcceptance::Recorded
        );
        assert_eq!(
            db.slack_requests(&next.workspace, &next.owner)
                .await
                .unwrap()
                .len(),
            2
        );
        let baseline = source("102.000", SlackRequestKind::Explicit);
        assert_eq!(
            db.accept_slack_source(&baseline, true).await.unwrap(),
            SlackSourceAcceptance::Recorded
        );
        assert_eq!(
            db.slack_requests(&next.workspace, &next.owner)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn duplicate_gestures_and_restart_retain_one_immutable_source() {
        let db = Database::open_in_memory().await.unwrap();
        let s = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&s, false).await.unwrap();
        let mut edited = s.clone();
        edited.instruction = "changed after acceptance".into();
        db.accept_slack_source(&edited, false).await.unwrap();
        let rows = db.slack_requests(&s.workspace, &s.owner).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source.instruction, "investigate");
        assert!(!rows[0].eyes_confirmed);
        db.slack_recover_returns().await.unwrap();
        assert_eq!(
            db.slack_requests(&s.workspace, &s.owner).await.unwrap()[0].request_id,
            rows[0].request_id
        );
    }
    #[tokio::test]
    async fn baseline_never_becomes_work_and_private_briefing_has_separate_binding() {
        let db = Database::open_in_memory().await.unwrap();
        let s = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&s, true).await.unwrap();
        db.accept_slack_source(&s, false).await.unwrap();
        assert!(db
            .slack_requests(&s.workspace, &s.owner)
            .await
            .unwrap()
            .is_empty());
        db.accept_slack_source(&source("101.000", SlackRequestKind::Explicit), false)
            .await
            .unwrap();
        db.accept_slack_source(&source("102.000", SlackRequestKind::Explicit), false)
            .await
            .unwrap();
        db.accept_slack_source(&source("103.000", SlackRequestKind::Briefing), false)
            .await
            .unwrap();
        let r = db.slack_requests(&s.workspace, &s.owner).await.unwrap();
        assert_eq!(r[0].binding_id, r[1].binding_id);
        assert_ne!(r[0].binding_id, r[2].binding_id);
    }
    #[tokio::test]
    async fn origin_columns_reject_unattributed_slack_and_preserve_native_history() {
        use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("test", "test", "/tmp", true, None, None)
            .await
            .unwrap();
        let s = source("100.000", SlackRequestKind::Explicit);
        db.accept_slack_source(&s, false).await.unwrap();
        let request = db
            .slack_requests(&s.workspace, &s.owner)
            .await
            .unwrap()
            .remove(0);
        let origin = InputOrigin::SlackGateway {
            request_id: request.request_id.clone(),
        };
        db.add_message_with_seq_and_origin(
            "input",
            "test",
            1,
            &MessageContent::user("source"),
            None,
            None,
            &origin,
        )
        .await
        .unwrap();
        assert_eq!(db.get_messages("test").await.unwrap()[0].origin, origin);
        sqlx::query("UPDATE messages SET origin_kind='remote_api',origin_source_instance_id='6fccba50-c1ac-43fd-bc13-7559428d2e90',origin_slack_request_id=NULL WHERE message_id='input'")
            .execute(db.pool()).await.unwrap();
        sqlx::query("UPDATE messages SET origin_kind='slack_gateway',origin_source_instance_id=NULL,origin_slack_request_id=?1 WHERE message_id='input'")
            .bind(&request.request_id)
            .execute(db.pool()).await.unwrap();
        assert_eq!(db.get_messages("test").await.unwrap()[0].origin, origin);
        assert!(sqlx::query(
            "UPDATE messages SET origin_slack_request_id=NULL WHERE message_id='input'"
        )
        .execute(db.pool())
        .await
        .is_err());
        assert!(
            sqlx::query("UPDATE messages SET origin_kind='user_api' WHERE message_id='input'")
                .execute(db.pool())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // Reuse one outbox across private-input rejection, shared coverage, and restart recovery.
    async fn shared_reply_has_one_delivery_and_exact_coverage_across_recovery() {
        use phoenix_core::domain::{
            db_schema::{InputOrigin, MessageContent, SourceToolCall},
            sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput},
        };
        let db = Database::open_in_memory().await.unwrap();
        let conv = db
            .create_conversation("test", "test", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut ids = vec![];
        for (i, ts) in ["100.000", "101.000"].iter().enumerate() {
            let s = source(ts, SlackRequestKind::Explicit);
            db.accept_slack_source(&s, false).await.unwrap();
            let r = db
                .slack_requests(&s.workspace, &s.owner)
                .await
                .unwrap()
                .pop()
                .unwrap();
            db.slack_bind_product(&r.binding_id, conv.product_conversation_id.as_str())
                .await
                .unwrap();
            db.add_message_with_seq_and_origin(
                &r.input_id,
                "test",
                i64::try_from(i + 1).unwrap(),
                &MessageContent::user("ask"),
                None,
                None,
                &InputOrigin::SlackGateway {
                    request_id: r.request_id.clone(),
                },
            )
            .await
            .unwrap();
            sqlx::query("INSERT INTO steering_acceptance_receipts(conversation_id,message_id,request_fingerprint) VALUES ('test',?1,'consumed-fixture')")
                .bind(&r.input_id).execute(db.pool()).await.unwrap();
            ids.push(r.request_id);
        }
        sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'fixture',1,1,1,0,0,'Active','fixture',1,X'00',1,1)").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,canonical_message_id,materialized) SELECT 1,1,'test','fixture','fixture',X'00','Runtime',0,1,input_id,1 FROM slack_gateway_requests WHERE request_id=?1").bind(&ids[0]).execute(db.pool()).await.unwrap();
        db.add_message_with_seq_and_origin(
            "private-input",
            "test",
            5,
            &MessageContent::user("Private follow-up"),
            None,
            None,
            &InputOrigin::UserApi,
        )
        .await
        .unwrap();
        // Native tool execution buffers the assistant in state until its checkpoint.
        let source = SourceToolCall {
            message_id: "assistant".into(),
            tool_use_id: "reply-tool".into(),
        };
        db.update_conversation_state(
            "test",
            &ConvState::ToolExecuting {
                current_tool: ToolCall::new(
                    "reply-tool",
                    ToolInput::Unknown {
                        name: "slack_gateway_reply".into(),
                        input: serde_json::json!({}),
                    },
                ),
                remaining_tools: vec![],
                completed_results: vec![],
                pending_sub_agents: vec![],
                assistant_message: AssistantMessage {
                    message_id: "assistant".into(),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
        let active: String = sqlx::query_scalar(
            "SELECT message_id FROM steering_execution_occurrences WHERE conversation_id='test'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(active, "private-input");
        assert!(db
            .queue_slack_reply(
                conv.product_conversation_id.as_str(),
                &ids,
                "test",
                &source,
                "Private material"
            )
            .await
            .is_err());
        sqlx::query("DELETE FROM durable_turns WHERE turn_id=1")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM workflows WHERE workflow_id=1")
            .execute(db.pool())
            .await
            .unwrap();
        let first_input: String =
            sqlx::query_scalar("SELECT input_id FROM slack_gateway_requests WHERE request_id=?1")
                .bind(&ids[0])
                .fetch_one(db.pool())
                .await
                .unwrap();
        sqlx::query(
            "UPDATE steering_execution_occurrences SET message_id=?1 WHERE conversation_id='test'",
        )
        .bind(first_input)
        .execute(db.pool())
        .await
        .unwrap();
        let id = db
            .queue_slack_reply(
                conv.product_conversation_id.as_str(),
                &ids,
                "test",
                &source,
                "answer",
            )
            .await
            .unwrap();
        sqlx::query("DELETE FROM steering_execution_occurrences WHERE conversation_id='test'")
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            db.queue_slack_reply(
                conv.product_conversation_id.as_str(),
                &ids,
                "test",
                &source,
                "answer",
            )
            .await
            .unwrap(),
            id
        );
        assert!(db
            .queue_slack_reply(
                conv.product_conversation_id.as_str(),
                &ids,
                "test",
                &SourceToolCall {
                    tool_use_id: "duplicate".into(),
                    ..source.clone()
                },
                "duplicate answer",
            )
            .await
            .is_err());
        assert!(db
            .queue_slack_reply(
                conv.product_conversation_id.as_str(),
                &ids[..1],
                "test",
                &source,
                "answer",
            )
            .await
            .is_err());
        assert!(db.slack_begin_return(&id, "attempt").await.unwrap());
        db.slack_recover_returns().await.unwrap();
        assert!(!db.slack_begin_return(&id, "blind-retry").await.unwrap());
        assert_eq!(
            db.slack_returns("example.slack.com", "UOWNER")
                .await
                .unwrap()[0]
                .state
                .as_str(),
            "uncertain"
        );
    }
}
