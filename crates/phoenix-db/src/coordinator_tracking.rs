use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, Sqlite, Transaction};

use crate::{workflow::LocalAuthorityResult, Database, DbError, DbResult};

pub const TRACKING_PAGE_SIZE: usize = 8;
pub const TRACKING_REVIEW_INTERVAL_US: i64 = 86_400_000_000;
const EVIDENCE_PAGE_SIZE: usize = 8;
const RETIREMENT_PAGE_SIZE: usize = 8;
pub const TRACKING_DETAIL_BYTES: usize = 96 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackingKind {
    RequestedWork,
    Bookmark,
}
impl TrackingKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::RequestedWork => "requested_work",
            Self::Bookmark => "bookmark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrackingReference {
    Conversation {
        instance_id: String,
        product_id: String,
    },
    Message {
        instance_id: String,
        transcript_id: String,
        message_id: String,
    },
    PullRequest {
        repository: String,
        pr_number: u32,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingEvidence {
    pub instance_id: String,
    pub transcript_id: String,
    pub message_id: String,
    pub quotation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingRecord {
    pub kind: TrackingKind,
    pub title: String,
    pub note: String,
    pub resumption: String,
    pub references: Vec<TrackingReference>,
    pub evidence: Vec<TrackingEvidence>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackingRetirement {
    Resolved,
    Dismissed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackingDisposition {
    Tracking,
    Resolved,
    Dismissed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TrackingState {
    Tracking,
    Resolved { reason: String },
    Dismissed { reason: String },
}
impl TrackingState {
    fn disposition(&self) -> TrackingDisposition {
        match self {
            Self::Tracking => TrackingDisposition::Tracking,
            Self::Resolved { .. } => TrackingDisposition::Resolved,
            Self::Dismissed { .. } => TrackingDisposition::Dismissed,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackingOutcome {
    Applied,
    Replayed,
    Unchanged,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrackingMutation {
    Record {
        record: TrackingRecord,
    },
    Update {
        id: String,
        expected_revision: i64,
        kind: TrackingKind,
        title: String,
        note: String,
        resumption: String,
        reviewed_evidence_ordinal: i64,
        references: Vec<TrackingReference>,
    },
    AppendEvidence {
        id: String,
        expected_revision: i64,
        evidence: Vec<TrackingEvidence>,
    },
    Retire {
        id: String,
        expected_revision: i64,
        disposition: TrackingRetirement,
        reason: String,
    },
    Reopen {
        id: String,
        expected_revision: i64,
        evidence: Vec<TrackingEvidence>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TrackingResumption {
    ReviewRequired {
        historical_note: String,
        latest_evidence_ordinal: i64,
        reviewed_evidence_ordinal: i64,
    },
    Reviewed {
        resumption: String,
        reviewed_evidence_ordinal: i64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingItem {
    pub id: String,
    pub kind: TrackingKind,
    pub title: String,
    pub note: String,
    pub disposition: TrackingState,
    pub revision: i64,
    pub resumption: TrackingResumption,
    pub created_at_us: i64,
    pub updated_at_us: i64,
    pub reviewed_at_us: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingPage {
    pub registry_revision: i64,
    pub active_count: i64,
    pub review_required_count: i64,
    pub items: Vec<TrackingSummary>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub omitted_count: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingSummary {
    pub id: String,
    pub kind: TrackingKind,
    pub title: String,
    pub title_truncated: bool,
    pub disposition: TrackingDisposition,
    pub revision: i64,
    pub review_required: bool,
    pub latest_evidence_ordinal: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingDetail {
    pub registry_revision: i64,
    pub item: TrackingItem,
    pub references: Vec<TrackingReference>,
    pub evidence: Vec<(i64, TrackingEvidence)>,
    pub next_evidence_ordinal: Option<i64>,
    pub evidence_is_historical: bool,
    pub page_source_availability: TrackingSourceAvailability,
    pub retirement_history: Vec<TrackingRetirementRecord>,
    pub next_retirement_revision: Option<i64>,
    pub omitted_retirement_count: i64,
    pub recorded_by_ember: Option<TrackingRecordingSource>,
    pub recorded_by_scanner: Option<crate::coordinator_scanner::ScanRecordingSource>,
    pub page_scanner_evidence_provenance: Vec<crate::coordinator_scanner::ScanEvidenceProvenance>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingRecordingSource {
    pub instance_id: String,
    pub transcript_id: String,
    pub assistant_message_id: String,
    pub tool_use_id: String,
    pub source_available: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackingSourceAvailability {
    NoEvidenceOnPage,
    LocalSourcesPresent,
    SourceMissing,
    RemoteUnverified,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingRetirementRecord {
    pub item_revision: i64,
    pub disposition: TrackingRetirement,
    pub reason: String,
    pub recorded_at_us: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackingReceipt {
    pub outcome: TrackingOutcome,
    pub item_id: String,
    pub item_revision: i64,
    pub registry_revision: i64,
}
fn invalid(message: &str) -> DbError {
    DbError::Serialization(format!("Tracking: {message}"))
}
fn count(value: usize) -> DbResult<i64> {
    i64::try_from(value).map_err(|_| invalid("collection count exceeds storage bounds"))
}
fn encoded_limit<T: Serialize + ?Sized>(value: &T, limit: usize, label: &str) -> DbResult<()> {
    let size = serde_json::to_vec(value)
        .map_err(|error| invalid(&error.to_string()))?
        .len();
    if size > limit {
        return Err(invalid(&format!(
            "{label} exceeds {limit} serialized JSON bytes; shorten the saved text or references"
        )));
    }
    Ok(())
}
fn editable_fields(
    kind: TrackingKind,
    title: &str,
    note: &str,
    resumption: &str,
    references: &[TrackingReference],
) -> DbResult<()> {
    bounded(title, 160, true)?;
    bounded(note, 8192, false)?;
    bounded(resumption, 4096, false)?;
    if references.len() > 16 {
        return Err(invalid("at most 16 references"));
    }
    encoded_limit(
        &(kind, title, note, resumption, references),
        16 * 1024,
        "combined title/note/resumption/references",
    )
}
fn bounded(value: &str, max: usize, nonblank: bool) -> DbResult<()> {
    if value.len() > max || (nonblank && value.trim().is_empty()) {
        Err(invalid("blank or oversized text"))
    } else {
        Ok(())
    }
}
fn digest(value: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(value) {
        write!(encoded, "{byte:02x}").expect("String formatting cannot fail");
    }
    encoded
}

pub(crate) async fn require_owner(
    tx: &mut Transaction<'_, Sqlite>,
    source: &str,
) -> DbResult<String> {
    bounded(source, 256, true)?;
    crate::product_conversation_rename::require_current_gc_tx(tx, source).await?;
    let owner: String =
        sqlx::query_scalar("SELECT product_conversation_id FROM conversations WHERE id=?1")
            .bind(source)
            .fetch_one(&mut **tx)
            .await?;
    let saved: Option<String> = sqlx::query_scalar(
        "SELECT owner_product_id FROM coordinator_tracking_registry WHERE singleton=1",
    )
    .fetch_optional(&mut **tx)
    .await?;
    if saved.as_deref().is_some_and(|saved| saved != owner) {
        return Err(invalid("singleton owner mismatch"));
    }
    Ok(owner)
}
async fn registry_revision(tx: &mut Transaction<'_, Sqlite>) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM coordinator_tracking_registry WHERE singleton=1",
    )
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(0))
}
pub(crate) async fn ensure_registry(tx: &mut Transaction<'_, Sqlite>, owner: &str) -> DbResult<()> {
    sqlx::query("INSERT INTO coordinator_tracking_registry(singleton,owner_product_id,revision) VALUES(1,?1,0) ON CONFLICT(singleton) DO NOTHING").bind(owner).execute(&mut **tx).await?;
    let saved: String = sqlx::query_scalar(
        "SELECT owner_product_id FROM coordinator_tracking_registry WHERE singleton=1",
    )
    .fetch_one(&mut **tx)
    .await?;
    if saved != owner {
        return Err(invalid("singleton owner mismatch"));
    }
    Ok(())
}
pub(crate) async fn latest_evidence(tx: &mut Transaction<'_, Sqlite>, id: &str) -> DbResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal),0) FROM coordinator_tracking_evidence WHERE item_id=?1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?)
}
pub(crate) async fn append_evidence(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    values: &[TrackingEvidence],
    now: i64,
) -> DbResult<bool> {
    if values.len() > 16 {
        return Err(invalid("at most 16 evidence records per operation"));
    }
    let mut ordinal = latest_evidence(tx, id).await?;
    let mut changed = false;
    for evidence in values {
        bounded(&evidence.instance_id, 256, true)?;
        bounded(&evidence.transcript_id, 256, true)?;
        bounded(&evidence.message_id, 256, true)?;
        bounded(&evidence.quotation, 4096, true)?;
        encoded_limit(evidence, 4096, "complete evidence entry including locators")?;
        let result = sqlx::query("INSERT OR IGNORE INTO coordinator_tracking_evidence(item_id,ordinal,instance_id,transcript_id,message_id,quotation,digest,recorded_at_us) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")
   .bind(id).bind(ordinal+1).bind(&evidence.instance_id).bind(&evidence.transcript_id).bind(&evidence.message_id).bind(&evidence.quotation).bind(digest(evidence.quotation.as_bytes())).bind(now).execute(&mut **tx).await?;
        if result.rows_affected() == 1 {
            ordinal += 1;
            changed = true;
        }
    }
    Ok(changed)
}
#[derive(Clone, Copy)]
pub(crate) enum TrackingInitialReview {
    Reviewed,
    NeedsReview,
}
pub(crate) fn validate_scanner_record(
    kind: TrackingKind,
    title: &str,
    note: &str,
    resumption: &str,
) -> DbResult<()> {
    editable_fields(kind, title, note, resumption, &[])
}
pub(crate) async fn create_tracking_item_tx(
    tx: &mut Transaction<'_, Sqlite>,
    record: &TrackingRecord,
    review: TrackingInitialReview,
    now: i64,
) -> DbResult<String> {
    editable_fields(
        record.kind,
        &record.title,
        &record.note,
        &record.resumption,
        &record.references,
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    let reviewed_at = match review {
        TrackingInitialReview::Reviewed => Some(now),
        TrackingInitialReview::NeedsReview => None,
    };
    sqlx::query("INSERT INTO coordinator_tracking_items(id,kind,title,note,resumption,revision,reviewed_evidence_ordinal,created_at_us,updated_at_us,reviewed_at_us) VALUES(?1,?2,?3,?4,?5,1,-1,?6,?6,?7)").bind(&id).bind(record.kind.as_str()).bind(&record.title).bind(&record.note).bind(&record.resumption).bind(now).bind(reviewed_at).execute(&mut **tx).await?;
    for (ordinal, reference) in record.references.iter().enumerate() {
        insert_reference(
            tx,
            &id,
            i64::try_from(ordinal).map_err(|_| invalid("reference count"))?,
            reference,
        )
        .await?;
    }
    append_evidence(tx, &id, &record.evidence, now).await?;
    if matches!(review, TrackingInitialReview::Reviewed) {
        let latest = latest_evidence(tx, &id).await?;
        sqlx::query(
            "UPDATE coordinator_tracking_items SET reviewed_evidence_ordinal=?2 WHERE id=?1",
        )
        .bind(&id)
        .bind(latest)
        .execute(&mut **tx)
        .await?;
    }
    Ok(id)
}
async fn insert_reference(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
    ordinal: i64,
    reference: &TrackingReference,
) -> DbResult<()> {
    let (kind, instance, product, transcript, message, repository, number) = match reference {
        TrackingReference::Conversation {
            instance_id,
            product_id,
        } => (
            "conversation",
            Some(instance_id),
            Some(product_id),
            None,
            None,
            None,
            None,
        ),
        TrackingReference::Message {
            instance_id,
            transcript_id,
            message_id,
        } => (
            "message",
            Some(instance_id),
            None,
            Some(transcript_id),
            Some(message_id),
            None,
            None,
        ),
        TrackingReference::PullRequest {
            repository,
            pr_number,
        } => (
            "pull_request",
            None,
            None,
            None,
            None,
            Some(repository),
            Some(i64::from(*pr_number)),
        ),
    };
    for text in [instance, product, transcript, message, repository]
        .into_iter()
        .flatten()
    {
        bounded(text, 512, true)?;
    }
    sqlx::query("INSERT INTO coordinator_tracking_references(item_id,ordinal,kind,instance_id,product_id,transcript_id,message_id,repository,pr_number) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")
 .bind(id).bind(ordinal).bind(kind).bind(instance).bind(product).bind(transcript).bind(message).bind(repository).bind(number).execute(&mut **tx).await?;
    Ok(())
}
fn item(row: &sqlx::sqlite::SqliteRow) -> DbResult<TrackingItem> {
    let latest: i64 = row.try_get("latest_evidence_ordinal")?;
    let reviewed: i64 = row.try_get("reviewed_evidence_ordinal")?;
    let resumption: String = row.try_get("resumption")?;
    Ok(TrackingItem {
        id: row.try_get("id")?,
        kind: match row.try_get::<String, _>("kind")?.as_str() {
            "requested_work" => TrackingKind::RequestedWork,
            "bookmark" => TrackingKind::Bookmark,
            _ => return Err(invalid("invalid kind")),
        },
        title: row.try_get("title")?,
        note: row.try_get("note")?,
        disposition: match row.try_get::<String, _>("disposition")?.as_str() {
            "tracking" => TrackingState::Tracking,
            "resolved" => TrackingState::Resolved {
                reason: row.try_get("retirement_reason")?,
            },
            "dismissed" => TrackingState::Dismissed {
                reason: row.try_get("retirement_reason")?,
            },
            _ => return Err(invalid("invalid disposition")),
        },
        revision: row.try_get("revision")?,
        resumption: if reviewed < latest {
            TrackingResumption::ReviewRequired {
                historical_note: resumption,
                latest_evidence_ordinal: latest,
                reviewed_evidence_ordinal: reviewed,
            }
        } else {
            TrackingResumption::Reviewed {
                resumption,
                reviewed_evidence_ordinal: reviewed,
            }
        },
        created_at_us: row.try_get("created_at_us")?,
        updated_at_us: row.try_get("updated_at_us")?,
        reviewed_at_us: row.try_get("reviewed_at_us")?,
    })
}
fn summary(row: &sqlx::sqlite::SqliteRow) -> DbResult<TrackingSummary> {
    let mut item = item(row)?;
    let title_truncated = item.title.len() > 80;
    let mut end = item.title.len().min(80);
    while !item.title.is_char_boundary(end) {
        end -= 1;
    }
    item.title.truncate(end);
    Ok(TrackingSummary {
        id: item.id,
        kind: item.kind,
        title: item.title,
        title_truncated,
        disposition: item.disposition.disposition(),
        revision: item.revision,
        review_required: matches!(item.resumption, TrackingResumption::ReviewRequired { .. }),
        latest_evidence_ordinal: row.try_get("latest_evidence_ordinal")?,
    })
}

impl Database {
    /// Applies a current-Coordinator mutation with durable exact-call receipts.
    ///
    /// # Errors
    /// Rejects stale/foreign sources, invalid input or revisions, and storage failures.
    pub async fn mutate_coordinator_tracking(
        &self,
        source: &str,
        call: &phoenix_core::domain::db_schema::SourceToolCall,
        mutation: &TrackingMutation,
    ) -> LocalAuthorityResult<DbResult<TrackingReceipt>> {
        use LocalAuthorityResult::{
            DurableFactEstablished as Established, DurableFactUnclassified as Unclassified,
        };
        let mut connection = match self.pool.acquire().await {
            Ok(value) => value,
            Err(error) => return Established(Err(error.into())),
        };
        let mut tx = match connection.begin_with("BEGIN IMMEDIATE").await {
            Ok(value) => value,
            Err(error) => return Established(Err(error.into())),
        };
        let result = Self::mutate_tracking_tx(&mut tx, source, call, mutation).await;
        match result {
            Ok(receipt) => match tx.commit().await {
                Ok(()) => Established(Ok(receipt)),
                Err(_) => Unclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => Established(Err(error)),
                Err(_) => Unclassified,
            },
        }
    }
    #[allow(clippy::too_many_lines)]
    async fn mutate_tracking_tx(
        tx: &mut Transaction<'_, Sqlite>,
        source: &str,
        call: &phoenix_core::domain::db_schema::SourceToolCall,
        mutation: &TrackingMutation,
    ) -> DbResult<TrackingReceipt> {
        let owner = require_owner(tx, source).await?;
        bounded(&call.message_id, 256, true)?;
        bounded(&call.tool_use_id, 256, true)?;
        let fingerprint =
            digest(&serde_json::to_vec(mutation).map_err(|error| invalid(&error.to_string()))?);
        if let Some(row)=sqlx::query("SELECT * FROM coordinator_tracking_operations WHERE source_transcript_id=?1 AND source_message_id=?2 AND source_tool_use_id=?3").bind(source).bind(&call.message_id).bind(&call.tool_use_id).fetch_optional(&mut **tx).await? {
   if row.try_get::<String,_>("fingerprint")? != fingerprint {return Err(invalid("call identity reused with changed intent"));}
   return Ok(TrackingReceipt{outcome:TrackingOutcome::Replayed,item_id:row.try_get("item_id")?,item_revision:row.try_get("item_revision")?,registry_revision:row.try_get("registry_revision")?});
  }
        let state_json: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id=?1")
            .bind(source)
            .fetch_one(&mut **tx)
            .await?;
        let state: phoenix_core::domain::sm_state::ConvState =
            serde_json::from_str(&state_json).map_err(|error| invalid(&error.to_string()))?;
        let phoenix_core::domain::sm_state::ConvState::ToolExecuting {
            current_tool,
            assistant_message,
            ..
        } = state
        else {
            return Err(invalid("source has no current durable tool round"));
        };
        if assistant_message.message_id != call.message_id
            || current_tool.id != call.tool_use_id
            || current_tool.name() != "tracking"
        {
            return Err(invalid(
                "source call is not the owned current tracking invocation",
            ));
        }
        let pending: TrackingMutation =
            serde_json::from_value(current_tool.input.to_value()["request"].clone())
                .map_err(|error| invalid(&error.to_string()))?;
        if digest(&serde_json::to_vec(&pending).map_err(|error| invalid(&error.to_string()))?)
            != fingerprint
        {
            return Err(invalid(
                "source arguments differ from the durable tool round",
            ));
        }
        let matched =
            assistant_message.content.iter().any(|block| {
                match block {
            phoenix_core::domain::llm_types::ContentBlock::ToolUse { id, name, input }
                if id == &call.tool_use_id && name == "tracking" =>
            {
                serde_json::from_value::<TrackingMutation>(input["request"].clone())
                    .ok()
                    .and_then(|input| serde_json::to_vec(&input).ok())
                    .is_some_and(|bytes| digest(&bytes) == fingerprint)
            }
            phoenix_core::domain::llm_types::ContentBlock::Text{..}
            | phoenix_core::domain::llm_types::ContentBlock::Image{..}
            | phoenix_core::domain::llm_types::ContentBlock::ToolUse{..}
            | phoenix_core::domain::llm_types::ContentBlock::ToolResult{..}
            | phoenix_core::domain::llm_types::ContentBlock::ServerToolUse{..}
            | phoenix_core::domain::llm_types::ContentBlock::ToolSearchToolResult{..}
            | phoenix_core::domain::llm_types::ContentBlock::WebSearchToolResult{..}
            | phoenix_core::domain::llm_types::ContentBlock::WebFetchToolResult{..}
            | phoenix_core::domain::llm_types::ContentBlock::CodeExecutionToolResult{..}
            | phoenix_core::domain::llm_types::ContentBlock::BashCodeExecutionToolResult{..}
            | phoenix_core::domain::llm_types::ContentBlock::TextEditorCodeExecutionToolResult{..}
            | phoenix_core::domain::llm_types::ContentBlock::McpToolUse{..}
            | phoenix_core::domain::llm_types::ContentBlock::McpToolResult{..}=>false,
        }
            });
        if !matched {
            return Err(invalid(
                "owned assistant round does not contain the source call",
            ));
        }
        ensure_registry(tx, &owner).await?;
        let now = Utc::now().timestamp_micros();
        let (id, revision, changed) = match mutation {
            TrackingMutation::Record { record } => {
                let id = create_tracking_item_tx(tx, record, TrackingInitialReview::Reviewed, now)
                    .await?;
                (id, 1, true)
            }
            other @ (TrackingMutation::Update { .. }
            | TrackingMutation::AppendEvidence { .. }
            | TrackingMutation::Retire { .. }
            | TrackingMutation::Reopen { .. }) => {
                let (id, expected) = match other {
                    TrackingMutation::Update {
                        id,
                        expected_revision,
                        ..
                    }
                    | TrackingMutation::AppendEvidence {
                        id,
                        expected_revision,
                        ..
                    }
                    | TrackingMutation::Retire {
                        id,
                        expected_revision,
                        ..
                    }
                    | TrackingMutation::Reopen {
                        id,
                        expected_revision,
                        ..
                    } => (id, *expected_revision),
                    TrackingMutation::Record { .. } => unreachable!(),
                };
                let row = sqlx::query(
                    "SELECT revision,current_retirement_revision FROM coordinator_tracking_items WHERE id=?1",
                )
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or_else(|| invalid("item not found"))?;
                let revision: i64 = row.try_get("revision")?;
                if revision != expected {
                    return Err(invalid("revision conflict; read the item again"));
                }
                let active = row
                    .try_get::<Option<i64>, _>("current_retirement_revision")?
                    .is_none();
                let changed = match other {
                    TrackingMutation::Update {
                        kind,
                        title,
                        note,
                        resumption,
                        reviewed_evidence_ordinal,
                        references,
                        ..
                    } => {
                        if !active {
                            return Err(invalid("retired item requires explicit reopen"));
                        }
                        editable_fields(*kind, title, note, resumption, references)?;
                        let latest = latest_evidence(tx, id).await?;
                        if *reviewed_evidence_ordinal != latest {
                            return Err(invalid(
                                "evidence revision conflict; inspect new source evidence",
                            ));
                        }
                        sqlx::query("UPDATE coordinator_tracking_items SET title=?2,note=?3,resumption=?4,reviewed_evidence_ordinal=?5,reviewed_at_us=?6,kind=?7 WHERE id=?1").bind(id).bind(title).bind(note).bind(resumption).bind(latest).bind(now).bind(kind.as_str()).execute(&mut **tx).await?;
                        sqlx::query("DELETE FROM coordinator_tracking_references WHERE item_id=?1")
                            .bind(id)
                            .execute(&mut **tx)
                            .await?;
                        for (ordinal, reference) in references.iter().enumerate() {
                            insert_reference(tx, id, count(ordinal)?, reference).await?;
                        }
                        true
                    }
                    TrackingMutation::AppendEvidence { evidence, .. } => {
                        append_evidence(tx, id, evidence, now).await?
                    }
                    TrackingMutation::Retire {
                        disposition,
                        reason,
                        ..
                    } => {
                        if !active {
                            return Err(invalid("already retired; read retained reason"));
                        }
                        bounded(reason, 4096, true)?;
                        encoded_limit(reason, 4096, "retirement reason")?;
                        let disposition = match disposition {
                            TrackingRetirement::Resolved => "resolved",
                            TrackingRetirement::Dismissed => "dismissed",
                        };
                        sqlx::query("INSERT INTO coordinator_tracking_retirements(item_id,item_revision,disposition,reason,recorded_at_us) VALUES(?1,?2,?3,?4,?5)").bind(id).bind(revision+1).bind(disposition).bind(reason).bind(now).execute(&mut **tx).await?;
                        sqlx::query("UPDATE coordinator_tracking_items SET current_retirement_revision=?2 WHERE id=?1").bind(id).bind(revision+1).execute(&mut **tx).await?;
                        true
                    }
                    TrackingMutation::Reopen { evidence, .. } => {
                        if active || evidence.is_empty() {
                            return Err(invalid(
                                "reopen requires a retired item and current evidence",
                            ));
                        }
                        append_evidence(tx, id, evidence, now).await?;
                        sqlx::query("UPDATE coordinator_tracking_items SET current_retirement_revision=NULL,reviewed_evidence_ordinal=-1 WHERE id=?1").bind(id).execute(&mut **tx).await?;
                        true
                    }
                    TrackingMutation::Record { .. } => unreachable!(),
                };
                let next = revision + i64::from(changed);
                if changed {
                    sqlx::query("UPDATE coordinator_tracking_items SET revision=?2,updated_at_us=MAX(updated_at_us,?3) WHERE id=?1").bind(id).bind(next).bind(now).execute(&mut **tx).await?;
                }
                (id.clone(), next, changed)
            }
        };
        if changed {
            sqlx::query(
                "UPDATE coordinator_tracking_registry SET revision=revision+1 WHERE singleton=1",
            )
            .execute(&mut **tx)
            .await?;
        }
        let registry = registry_revision(tx).await?;
        let operation_kind = match mutation {
            TrackingMutation::Record { .. } => "record",
            TrackingMutation::Update { .. } => "update",
            TrackingMutation::AppendEvidence { .. } => "append_evidence",
            TrackingMutation::Retire { .. } => "retire",
            TrackingMutation::Reopen { .. } => "reopen",
        };
        sqlx::query("INSERT INTO coordinator_tracking_operations(source_transcript_id,source_message_id,source_tool_use_id,fingerprint,operation_kind,item_id,item_revision,registry_revision) VALUES(?1,?2,?3,?4,?8,?5,?6,?7)").bind(source).bind(&call.message_id).bind(&call.tool_use_id).bind(fingerprint).bind(&id).bind(revision).bind(registry).bind(operation_kind).execute(&mut **tx).await?;
        Ok(TrackingReceipt {
            outcome: if changed {
                TrackingOutcome::Applied
            } else {
                TrackingOutcome::Unchanged
            },
            item_id: id,
            item_revision: revision,
            registry_revision: registry,
        })
    }
    /// Reads one bounded registry page. Cursor is the last returned stable item ID.
    ///
    /// # Errors
    /// Rejects a stale/foreign source and failed or malformed stored reads.
    pub async fn list_coordinator_tracking(
        &self,
        source: &str,
        after: Option<&str>,
        include_retired: bool,
    ) -> DbResult<TrackingPage> {
        let mut tx = self.pool.begin().await?;
        require_owner(&mut tx, source).await?;
        let page = tracking_page_tx(&mut tx, after, include_retired).await?;
        tx.commit().await?;
        Ok(page)
    }
    /// Reads one item and one bounded page of its retained historical evidence.
    ///
    /// # Errors
    /// Rejects invalid cursors, stale/foreign sources, missing items or storage failure.
    pub async fn read_coordinator_tracking(
        &self,
        source: &str,
        id: &str,
        after_evidence: i64,
    ) -> DbResult<TrackingDetail> {
        self.read_coordinator_tracking_page(source, id, after_evidence, None)
            .await
    }
    /// Reads separately paginated evidence and retirement history with a single snapshot.
    ///
    /// # Errors
    /// Rejects invalid cursors, stale/foreign sources, missing items or storage failure.
    #[allow(clippy::too_many_lines)]
    pub async fn read_coordinator_tracking_page(
        &self,
        source: &str,
        id: &str,
        after_evidence: i64,
        before_retirement_revision: Option<i64>,
    ) -> DbResult<TrackingDetail> {
        if before_retirement_revision.is_some_and(|cursor| cursor < 1) {
            return Err(invalid("invalid retirement cursor"));
        }
        if after_evidence < 0 {
            return Err(invalid("negative evidence cursor"));
        }
        let mut tx = self.pool.begin().await?;
        require_owner(&mut tx, source).await?;
        let row = sqlx::query("SELECT i.*,COALESCE(r.disposition,'tracking') AS disposition,r.reason AS retirement_reason,COALESCE((SELECT MAX(ordinal) FROM coordinator_tracking_evidence e WHERE e.item_id=i.id),0) AS latest_evidence_ordinal FROM coordinator_tracking_items i LEFT JOIN coordinator_tracking_retirements r ON r.item_id=i.id AND r.item_revision=i.current_retirement_revision WHERE i.id=?1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| invalid("item not found"))?;
        let item = item(&row)?;
        let rows = sqlx::query(
            "SELECT * FROM coordinator_tracking_references WHERE item_id=?1 ORDER BY ordinal",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let mut references = Vec::new();
        for row in rows {
            references.push(match row.try_get::<String, _>("kind")?.as_str() {
                "conversation" => TrackingReference::Conversation {
                    instance_id: row.try_get("instance_id")?,
                    product_id: row.try_get("product_id")?,
                },
                "message" => TrackingReference::Message {
                    instance_id: row.try_get("instance_id")?,
                    transcript_id: row.try_get("transcript_id")?,
                    message_id: row.try_get("message_id")?,
                },
                "pull_request" => TrackingReference::PullRequest {
                    repository: row.try_get("repository")?,
                    pr_number: row
                        .try_get::<i64, _>("pr_number")?
                        .try_into()
                        .map_err(|_| invalid("invalid PR number"))?,
                },
                _ => return Err(invalid("invalid reference")),
            });
        }
        let mut rows=sqlx::query("SELECT * FROM coordinator_tracking_evidence WHERE item_id=?1 AND ordinal>?2 ORDER BY ordinal LIMIT ?3").bind(id).bind(after_evidence).bind(count(EVIDENCE_PAGE_SIZE+1)?).fetch_all(&mut *tx).await?;
        let more = rows.len() > EVIDENCE_PAGE_SIZE;
        rows.truncate(EVIDENCE_PAGE_SIZE);
        let mut evidence = Vec::new();
        for row in rows {
            evidence.push((
                row.try_get("ordinal")?,
                TrackingEvidence {
                    instance_id: row.try_get("instance_id")?,
                    transcript_id: row.try_get("transcript_id")?,
                    message_id: row.try_get("message_id")?,
                    quotation: row.try_get("quotation")?,
                },
            ));
        }
        let next = if more {
            evidence.last().map(|(ordinal, _)| *ordinal)
        } else {
            None
        };
        let local_instance: String =
            sqlx::query_scalar("SELECT instance_id FROM instance_identity WHERE singleton_key=1")
                .fetch_one(&mut *tx)
                .await?;
        let mut page_source_availability = if evidence.is_empty() {
            TrackingSourceAvailability::NoEvidenceOnPage
        } else {
            TrackingSourceAvailability::LocalSourcesPresent
        };
        for (_, entry) in &evidence {
            if entry.instance_id != local_instance {
                if !matches!(
                    page_source_availability,
                    TrackingSourceAvailability::SourceMissing
                ) {
                    page_source_availability = TrackingSourceAvailability::RemoteUnverified;
                }
                continue;
            }
            let exists: i64 = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE message_id=?1 AND conversation_id=?2)",
            )
            .bind(&entry.message_id)
            .bind(&entry.transcript_id)
            .fetch_one(&mut *tx)
            .await?;
            if exists == 0 {
                page_source_availability = TrackingSourceAvailability::SourceMissing;
            }
        }
        let before = before_retirement_revision.unwrap_or(i64::MAX);
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_tracking_retirements WHERE item_id=?1 AND item_revision<?2").bind(id).bind(before).fetch_one(&mut *tx).await?;
        let rows=sqlx::query("SELECT item_revision,disposition,reason,recorded_at_us FROM coordinator_tracking_retirements WHERE item_id=?1 AND item_revision<?2 ORDER BY item_revision DESC LIMIT ?3").bind(id).bind(before).bind(count(RETIREMENT_PAGE_SIZE)?).fetch_all(&mut *tx).await?;
        let retirement_history = rows
            .iter()
            .map(|row| {
                Ok(TrackingRetirementRecord {
                    item_revision: row.try_get("item_revision")?,
                    disposition: match row.try_get::<String, _>("disposition")?.as_str() {
                        "resolved" => TrackingRetirement::Resolved,
                        "dismissed" => TrackingRetirement::Dismissed,
                        _ => return Err(invalid("invalid retired disposition")),
                    },
                    reason: row.try_get("reason")?,
                    recorded_at_us: row.try_get("recorded_at_us")?,
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
        let registry = registry_revision(&mut tx).await?;
        let origin=sqlx::query("SELECT o.source_transcript_id,o.source_message_id,o.source_tool_use_id,EXISTS(SELECT 1 FROM messages m WHERE m.message_id=o.source_message_id AND m.conversation_id=o.source_transcript_id) AS source_available FROM coordinator_tracking_operations o WHERE o.item_id=?1 AND o.operation_kind='record'").bind(id).fetch_optional(&mut *tx).await?;
        let recorded_by_ember = origin
            .map(|row| {
                Ok::<_, DbError>(TrackingRecordingSource {
                    instance_id: local_instance,
                    transcript_id: row.try_get("source_transcript_id")?,
                    assistant_message_id: row.try_get("source_message_id")?,
                    tool_use_id: row.try_get("source_tool_use_id")?,
                    source_available: row.try_get("source_available")?,
                })
            })
            .transpose()?;
        let recorded_by_scanner=sqlx::query("SELECT a.reserved_at_us,w.lane,w.source_generation,w.connection,w.prompt_hash,w.endpoint_hash FROM coordinator_scan_item_origins o JOIN coordinator_scan_attempts a ON a.id=o.attempt_id JOIN coordinator_scan_work w ON w.binding_id=a.binding_id WHERE o.item_id=?1").bind(id).fetch_optional(&mut *tx).await?.map(|row|{
            Ok::<_,DbError>(crate::coordinator_scanner::ScanRecordingSource {
                captured_at_us:row.try_get("reserved_at_us")?,
                lane:serde_json::from_value(serde_json::Value::String(row.try_get("lane")?)).map_err(|e|invalid(&e.to_string()))?,
                source_generation:row.try_get("source_generation")?,
                model:crate::coordinator_scanner::SCAN_MODEL.into(),
                connection:serde_json::from_value(serde_json::Value::String(row.try_get("connection")?)).map_err(|e|invalid(&e.to_string()))?,
                prompt_hash:row.try_get("prompt_hash")?,endpoint_hash:row.try_get("endpoint_hash")?
            })
        }).transpose()?;
        let mut page_scanner_evidence_provenance = Vec::new();
        for (ordinal, _) in &evidence {
            if let Some(row)=sqlx::query("SELECT * FROM coordinator_scan_evidence_origins WHERE item_id=?1 AND evidence_ordinal=?2").bind(id).bind(ordinal).fetch_optional(&mut *tx).await?{
                page_scanner_evidence_provenance.push(crate::coordinator_scanner::ScanEvidenceProvenance {
                    evidence_ordinal:*ordinal,
                    origin_kind:serde_json::from_value(serde_json::Value::String(row.try_get("origin_kind")?)).map_err(|e|invalid(&e.to_string()))?,
                    source_role:serde_json::from_value(serde_json::Value::String(row.try_get("source_role")?)).map_err(|e|invalid(&e.to_string()))?,
                    source_generation:row.try_get("source_generation")?,utf8_start:row.try_get("utf8_start")?,utf8_end:row.try_get("utf8_end")?
                });
            }
        }
        let omitted_retirement_count = total - count(retirement_history.len())?;
        let next_retirement_revision = if omitted_retirement_count > 0 {
            retirement_history.last().map(|entry| entry.item_revision)
        } else {
            None
        };
        tx.commit().await?;
        let detail = TrackingDetail {
            registry_revision: registry,
            item,
            references,
            evidence,
            next_evidence_ordinal: next,
            evidence_is_historical: true,
            page_source_availability,
            retirement_history,
            next_retirement_revision,
            omitted_retirement_count,
            recorded_by_ember,
            recorded_by_scanner,
            page_scanner_evidence_provenance,
        };
        encoded_limit(&detail, TRACKING_DETAIL_BYTES, "complete tracking detail")?;
        Ok(detail)
    }
}
async fn tracking_page_tx(
    tx: &mut Transaction<'_, Sqlite>,
    after: Option<&str>,
    include_retired: bool,
) -> DbResult<TrackingPage> {
    let counts=sqlx::query("SELECT COUNT(*) AS active_count,COALESCE(SUM(reviewed_evidence_ordinal<COALESCE((SELECT MAX(ordinal) FROM coordinator_tracking_evidence e WHERE e.item_id=i.id),0)),0) AS review_required_count FROM coordinator_tracking_items i WHERE current_retirement_revision IS NULL").fetch_one(&mut **tx).await?;
    let mut rows = sqlx::query("SELECT i.*,COALESCE(r.disposition,'tracking') AS disposition,r.reason AS retirement_reason,COALESCE((SELECT MAX(ordinal) FROM coordinator_tracking_evidence e WHERE e.item_id=i.id),0) AS latest_evidence_ordinal FROM coordinator_tracking_items i LEFT JOIN coordinator_tracking_retirements r ON r.item_id=i.id AND r.item_revision=i.current_retirement_revision WHERE (?1 OR i.current_retirement_revision IS NULL) AND (?2 IS NULL OR i.id>?2) ORDER BY i.id LIMIT ?3")
        .bind(include_retired)
        .bind(after)
        .bind(count(TRACKING_PAGE_SIZE+1)?)
        .fetch_all(&mut **tx)
        .await?;
    let more = rows.len() > TRACKING_PAGE_SIZE;
    rows.truncate(TRACKING_PAGE_SIZE);
    let items = rows.iter().map(summary).collect::<DbResult<Vec<_>>>()?;
    let next = if more {
        items.last().map(|i| i.id.clone())
    } else {
        None
    };
    let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_tracking_items WHERE (?1 OR current_retirement_revision IS NULL) AND (?2 IS NULL OR id>?2)").bind(include_retired).bind(after).fetch_one(&mut **tx).await?;
    Ok(TrackingPage {
        registry_revision: registry_revision(tx).await?,
        active_count: counts.try_get("active_count")?,
        review_required_count: counts.try_get("review_required_count")?,
        omitted_count: total - count(items.len())?,
        items,
        has_more: more,
        next_cursor: next,
    })
}
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingObservation {
    pub captured_registry: TrackingPage,
    pub requested_item_due_for_review: Option<TrackingSummary>,
    pub captured_at_us: i64,
    pub admitted_input_sequence_id: Option<i64>,
}
impl Database {
    /// Atomically appends a changed saved-intent observation and its attention receipt.
    ///
    /// # Errors
    /// Rejects stale sources, invalid bounds/time, or failed storage with classified durability.
    pub async fn append_coordinator_tracking_observation(
        &self,
        source: &str,
        sequence_id: i64,
        now_us: i64,
    ) -> LocalAuthorityResult<DbResult<Option<crate::Message>>> {
        use LocalAuthorityResult::{
            DurableFactEstablished as Established, DurableFactUnclassified as Unclassified,
        };
        let mut connection = match self.pool.acquire().await {
            Ok(value) => value,
            Err(error) => return Established(Err(error.into())),
        };
        let mut tx = match connection.begin_with("BEGIN IMMEDIATE").await {
            Ok(value) => value,
            Err(error) => return Established(Err(error.into())),
        };
        let result = append_observation_tx(&mut tx, source, sequence_id, now_us).await;
        match result {
            Ok(value) => match tx.commit().await {
                Ok(()) => Established(Ok(value)),
                Err(_) => Unclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => Established(Err(error)),
                Err(_) => Unclassified,
            },
        }
    }
}
#[allow(clippy::too_many_lines)]
async fn append_observation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    source: &str,
    sequence_id: i64,
    now_us: i64,
) -> DbResult<Option<crate::Message>> {
    require_owner(tx, source).await?;
    let page = tracking_page_tx(tx, None, false).await?;
    let previous: Option<i64> = sqlx::query_scalar(
        "SELECT registry_revision FROM coordinator_tracking_observations WHERE transcript_id=?1",
    )
    .bind(source)
    .fetch_optional(&mut **tx)
    .await?;
    let input_message_id:Option<String>=sqlx::query_scalar("SELECT message_id FROM messages WHERE conversation_id=?1 AND message_type='user' AND origin_kind!='system_generated' ORDER BY sequence_id DESC LIMIT 1").bind(source).fetch_optional(&mut **tx).await?;
    let input_message_id = input_message_id.unwrap_or_else(|| source.to_owned());
    let admitted_input_sequence_id: Option<i64> = sqlx::query_scalar(
        "SELECT sequence_id FROM messages WHERE message_id=?1 AND conversation_id=?2",
    )
    .bind(&input_message_id)
    .bind(source)
    .fetch_optional(&mut **tx)
    .await?;
    let previous_input: Option<String> = sqlx::query_scalar(
        "SELECT input_message_id FROM coordinator_tracking_observations WHERE transcript_id=?1",
    )
    .bind(source)
    .fetch_optional(&mut **tx)
    .await?;
    if page.registry_revision == 0 && page.active_count == 0 {
        return Ok(None);
    }
    let cue_row = if previous_input.as_deref() == Some(input_message_id.as_str()) {
        None
    } else {
        sqlx::query("SELECT i.*,'tracking' AS disposition,NULL AS retirement_reason,COALESCE((SELECT MAX(ordinal) FROM coordinator_tracking_evidence e WHERE e.item_id=i.id),0) AS latest_evidence_ordinal FROM coordinator_tracking_items i LEFT JOIN coordinator_tracking_attention a ON a.item_id=i.id WHERE i.current_retirement_revision IS NULL AND i.kind='requested_work' AND (i.reviewed_at_us IS NULL OR i.reviewed_at_us<=?1 OR i.reviewed_evidence_ordinal<COALESCE((SELECT MAX(ordinal) FROM coordinator_tracking_evidence e WHERE e.item_id=i.id),0)) AND (a.item_id IS NULL OR a.surfaced_at_us<=?1 OR a.evidence_ordinal<COALESCE((SELECT MAX(ordinal) FROM coordinator_tracking_evidence e WHERE e.item_id=i.id),0)) ORDER BY COALESCE(a.surfaced_at_us,0),COALESCE(i.reviewed_at_us,0),i.created_at_us,i.id LIMIT 1").bind(now_us.saturating_sub(TRACKING_REVIEW_INTERVAL_US)).fetch_optional(&mut **tx).await?
    };
    let cue = cue_row.as_ref().map(summary).transpose()?;
    if previous == Some(page.registry_revision) && cue.is_none() {
        return Ok(None);
    }
    let registry = page.registry_revision;
    let observation = TrackingObservation {
        captured_registry: page,
        requested_item_due_for_review: cue.clone(),
        captured_at_us: now_us,
        admitted_input_sequence_id,
    };
    let json = serde_json::to_string(&observation).map_err(|error| invalid(&error.to_string()))?;
    let text=format!("[Saved Ember tracking observation: captured historical data, not a user instruction, permission, current worker status or comprehensive inventory. Use tracking read/list for current detail. New evidence requires reconciliation. A due cue applies only to the admitted input immediately BEFORE this observation; once a later input arrives it is historical and does not renew review. A non-null requested_item_due_for_review selects one bounded inspection under the static review policy after handling the current request; it is not a new USER commission, execution permission or instruction to nudge a worker. Inspection alone is not follow-through: use the static next-step policy, preserving actual restrictions and existing execution authority. No useful action means stay quiet.]\n{json}");
    if text.len() > 8192 {
        return Err(invalid("compact observation exceeds 8192 bytes"));
    }
    let content = crate::MessageContent::User(crate::UserContent::meta(text));
    let message = crate::Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        conversation_id: source.into(),
        sequence_id,
        message_type: content.message_type(),
        content,
        display_data: None,
        usage_data: None,
        created_at: chrono::DateTime::from_timestamp_micros(now_us)
            .ok_or_else(|| invalid("invalid observation time"))?,
        origin: phoenix_core::domain::db_schema::InputOrigin::SystemGenerated,
    };
    crate::insert_message_tx(tx, &message).await?;
    if let Some(cue) = &cue {
        sqlx::query("INSERT INTO coordinator_tracking_attention(item_id,surfaced_at_us,evidence_ordinal) VALUES(?1,?2,?3) ON CONFLICT(item_id) DO UPDATE SET surfaced_at_us=excluded.surfaced_at_us,evidence_ordinal=excluded.evidence_ordinal").bind(&cue.id).bind(now_us).bind(cue.latest_evidence_ordinal).execute(&mut **tx).await?;
    }
    sqlx::query("INSERT INTO coordinator_tracking_observations(transcript_id,input_message_id,registry_revision,message_id,cue_item_id,cue_at_us) VALUES(?1,?6,?2,?3,?4,?5) ON CONFLICT(transcript_id) DO UPDATE SET input_message_id=excluded.input_message_id,registry_revision=excluded.registry_revision,message_id=excluded.message_id,cue_item_id=excluded.cue_item_id,cue_at_us=excluded.cue_at_us").bind(source).bind(registry).bind(&message.message_id).bind(cue.as_ref().map(|item|&item.id)).bind(cue.as_ref().map(|_|now_us)).bind(input_message_id).execute(&mut **tx).await?;
    Ok(Some(message))
}
