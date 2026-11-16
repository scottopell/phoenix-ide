use super::{
    InputOrigin, ScanCandidate, ScanCandidateState, ScanFragment, ScanLane, ScanPosition, ScanRole,
    TrackingKind, TrackingRetirement,
};
use crate::{DbError, DbResult, MessageContent};
use phoenix_core::domain::llm_types::ContentBlock;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};

pub(super) fn invalid(value: impl Into<String>) -> DbError {
    DbError::Serialization(value.into())
}
pub(super) fn hex(bytes: impl AsRef<[u8]>) -> String {
    {
        use std::fmt::Write as _;
        bytes.as_ref().iter().fold(String::new(), |mut text, byte| {
            write!(text, "{byte:02x}").expect("String formatting cannot fail");
            text
        })
    }
}
pub(super) fn hash(bytes: impl AsRef<[u8]>) -> String {
    hex(Sha256::digest(bytes.as_ref()))
}
pub(super) fn encoded<T: Serialize + ?Sized>(value: &T) -> DbResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| invalid(e.to_string()))
}
pub(super) fn seq(position: &ScanPosition) -> i64 {
    match position {
        ScanPosition::Boundary { sequence_id } | ScanPosition::Fragment { sequence_id, .. } => {
            *sequence_id
        }
    }
}
pub(super) fn lane_name(lane: ScanLane) -> &'static str {
    match lane {
        ScanLane::Live => "live",
        ScanLane::Historical => "historical",
    }
}

macro_rules! chain_query { ($suffix:literal) => { concat!("WITH RECURSIVE chain(id, depth) AS (
 SELECT id,0 FROM conversations WHERE id=?1 AND product_conversation_id=?2
 AND runtime_role='coordinator' AND parent_conversation_id IS NULL
 UNION ALL SELECT p.id,chain.depth+1 FROM chain JOIN conversations p ON p.continued_in_conv_id=chain.id
 WHERE p.product_conversation_id=?2 AND p.runtime_role='coordinator' AND p.parent_conversation_id IS NULL
), members AS (
 SELECT c.id, (SELECT MAX(depth) FROM chain)-chain.depth AS ordinal,c.transcript_generation,
 COALESCE((SELECT MAX(sequence_id) FROM messages WHERE conversation_id=c.id),0) AS tail
 FROM chain JOIN conversations c ON c.id=chain.id) ", $suffix) }; }
pub(super) use chain_query;

pub(super) struct Topology {
    pub owner: String,
    pub head: String,
    pub maximum: i64,
}
#[derive(Debug)]
pub(super) struct Member {
    pub id: String,
    pub ordinal: i64,
    pub generation: i64,
    pub tail: i64,
}
fn member_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<Member> {
    Ok(Member {
        id: row.try_get("id")?,
        ordinal: row.try_get("ordinal")?,
        generation: row.try_get("transcript_generation")?,
        tail: row.try_get("tail")?,
    })
}
pub(super) async fn owner(tx: &mut Transaction<'_, Sqlite>) -> DbResult<Option<(String, String)>> {
    let head:Option<String> = sqlx::query_scalar("SELECT c.id FROM conversations c JOIN product_conversations p ON p.id=c.product_conversation_id WHERE c.coordinator_head=1 AND c.runtime_role='coordinator' AND c.parent_conversation_id IS NULL AND p.kind='coordinator' AND NOT EXISTS(SELECT 1 FROM product_creation_jobs j WHERE j.product_conversation_id=p.id AND j.status<>'published')").fetch_optional(&mut **tx).await?;
    let Some(head) = head else { return Ok(None) };
    let owner = crate::coordinator_tracking::require_owner(tx, &head).await?;
    Ok(Some((owner, head)))
}
pub(super) async fn topology(tx: &mut Transaction<'_, Sqlite>) -> DbResult<Option<Topology>> {
    let Some((owner, head)) = owner(tx).await? else {
        return Ok(None);
    };
    let maximum: i64 = sqlx::query_scalar(chain_query!(" SELECT MAX(ordinal) FROM members"))
        .bind(&head)
        .bind(&owner)
        .fetch_one(&mut **tx)
        .await?;
    Ok(Some(Topology {
        owner,
        head,
        maximum,
    }))
}
pub(super) async fn member_page(
    tx: &mut Transaction<'_, Sqlite>,
    topology: &Topology,
    start: i64,
) -> DbResult<Vec<Member>> {
    let rows = sqlx::query(chain_query!(
        " SELECT * FROM members WHERE ordinal>=?3 ORDER BY ordinal LIMIT 8"
    ))
    .bind(&topology.head)
    .bind(&topology.owner)
    .bind(start)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter().map(member_row).collect()
}
pub(super) async fn member(
    tx: &mut Transaction<'_, Sqlite>,
    topology: &Topology,
    id: &str,
) -> DbResult<Option<Member>> {
    let row = sqlx::query(chain_query!(" SELECT * FROM members WHERE id=?3"))
        .bind(&topology.head)
        .bind(&topology.owner)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    row.as_ref().map(member_row).transpose()
}
pub(super) fn position(row: &sqlx::sqlite::SqliteRow, lane: ScanLane) -> DbResult<ScanPosition> {
    let prefix = lane_name(lane);
    let sequence_id = row.try_get(format!("{prefix}_seq").as_str())?;
    let message_id: Option<String> = row.try_get(format!("{prefix}_message_id").as_str())?;
    match message_id {
        None => Ok(ScanPosition::Boundary { sequence_id }),
        Some(message_id) => Ok(ScanPosition::Fragment {
            sequence_id,
            message_id,
            part_ordinal: usize::try_from(
                row.try_get::<i64, _>(format!("{prefix}_part").as_str())?,
            )
            .map_err(|e| invalid(e.to_string()))?,
            utf8_offset: usize::try_from(
                row.try_get::<i64, _>(format!("{prefix}_offset").as_str())?,
            )
            .map_err(|e| invalid(e.to_string()))?,
            prose_digest: row.try_get(format!("{prefix}_digest").as_str())?,
        }),
    }
}
pub(super) async fn advance(
    tx: &mut Transaction<'_, Sqlite>,
    transcript: &str,
    lane: ScanLane,
    end: &ScanPosition,
) -> DbResult<()> {
    let (message, part, offset, digest) = match end {
        ScanPosition::Boundary { .. } => (None, None, None, None),
        ScanPosition::Fragment {
            message_id,
            part_ordinal,
            utf8_offset,
            prose_digest,
            ..
        } => (
            Some(message_id),
            Some(i64::try_from(*part_ordinal).map_err(|e| invalid(e.to_string()))?),
            Some(i64::try_from(*utf8_offset).map_err(|e| invalid(e.to_string()))?),
            Some(prose_digest),
        ),
    };
    sqlx::query(match lane { ScanLane::Live=>"UPDATE coordinator_scan_members SET live_seq=?2,live_message_id=?3,live_part=?4,live_offset=?5,live_digest=?6,frontier_revision=frontier_revision+1 WHERE transcript_id=?1",ScanLane::Historical=>"UPDATE coordinator_scan_members SET historical_seq=?2,historical_message_id=?3,historical_part=?4,historical_offset=?5,historical_digest=?6,frontier_revision=frontier_revision+1 WHERE transcript_id=?1"}).bind(transcript).bind(seq(end)).bind(message).bind(part).bind(offset).bind(digest).execute(&mut **tx).await?;
    Ok(())
}

pub(super) struct Captured {
    pub fragments: Vec<ScanFragment>,
    pub end: ScanPosition,
    pub source_digest: String,
}
pub(super) struct SourceReadAllowance {
    remaining: usize,
}
impl SourceReadAllowance {
    pub fn new() -> Self {
        Self { remaining: 32 }
    }
    pub fn remaining(&self) -> usize {
        self.remaining
    }
    fn charge(&mut self, fetched: usize) -> DbResult<()> {
        self.remaining = self
            .remaining
            .checked_sub(fetched)
            .ok_or_else(|| invalid("source read exceeded pass allowance"))?;
        Ok(())
    }
}
pub(super) async fn capture(
    tx: &mut Transaction<'_, Sqlite>,
    transcript: &str,
    generation: i64,
    lane: ScanLane,
    start: &ScanPosition,
    allowance: &mut SourceReadAllowance,
) -> DbResult<Captured> {
    capture_range(tx, transcript, generation, lane, start, None, allowance).await
}
#[allow(clippy::too_many_lines)]
pub(super) async fn capture_range(
    tx: &mut Transaction<'_, Sqlite>,
    transcript: &str,
    generation: i64,
    lane: ScanLane,
    start: &ScanPosition,
    through: Option<&ScanPosition>,
    allowance: &mut SourceReadAllowance,
) -> DbResult<Captured> {
    if allowance.remaining() == 0 {
        return Err(invalid("source read allowance exhausted"));
    }
    let mut query=sqlx::QueryBuilder::<Sqlite>::new("SELECT message_id,conversation_id,sequence_id,message_type,created_at,origin_kind,origin_product_conversation_id,origin_transcript_id,origin_subscription_event_id,origin_source_instance_id,origin_slack_request_id,origin_reminder_id,origin_source_message_id,origin_source_tool_use_id,CASE WHEN message_type IN('user','agent') THEN CAST(substr(CAST(content AS BLOB),1,1048577) AS TEXT) ELSE NULL END AS prose_content,length(CAST(content AS BLOB)) AS content_bytes,EXISTS(SELECT 1 FROM message_images WHERE message_images.message_id=messages.message_id) OR EXISTS(SELECT 1 FROM message_files WHERE message_files.message_id=messages.message_id) AS has_attachments FROM messages WHERE conversation_id=");
    query.push_bind(transcript);
    query.push(match (lane, start) {
        (ScanLane::Live, ScanPosition::Boundary { .. }) => " AND sequence_id>",
        (ScanLane::Live, _) => " AND sequence_id>=",
        (ScanLane::Historical, _) => " AND sequence_id<=",
    });
    query.push_bind(seq(start));
    if let Some(through) = through {
        query.push(match (lane, through) {
            (ScanLane::Live, _) => " AND sequence_id<=",
            (ScanLane::Historical, ScanPosition::Boundary { .. }) => " AND sequence_id>",
            (ScanLane::Historical, ScanPosition::Fragment { .. }) => " AND sequence_id>=",
        });
        query.push_bind(seq(through));
    }

    query.push(if lane == ScanLane::Live {
        " ORDER BY sequence_id,message_id LIMIT "
    } else {
        " ORDER BY sequence_id DESC,message_id DESC LIMIT "
    });
    query.push_bind(i64::try_from(allowance.remaining()).map_err(|e| invalid(e.to_string()))?);
    let rows = query.build().fetch_all(&mut **tx).await?;
    allowance.charge(rows.len())?;
    let mut fragments = Vec::new();
    let mut end = start.clone();
    let mut identities = Vec::new();
    for row in rows {
        let message = project_source(&row)?;
        let message_seq = message.sequence_id;
        let origin = message.origin;
        let (role, is_meta, parts) = match message.content {
            Some(MessageContent::User(user)) => {
                if user.llm_text.is_some() {
                    tracing::debug!(message_id=%message.message_id,"scanner consumes original prose, not reference expansion");
                }
                if user.is_meta
                    && matches!(origin, InputOrigin::SystemGenerated)
                    && user.text.starts_with("[Saved Ember tracking observation:")
                {
                    (ScanRole::User, true, Vec::new())
                } else {
                    (ScanRole::User, user.is_meta, vec![(0, user.text)])
                }
            }
            Some(MessageContent::Agent(blocks)) => {
                let mut parts = Vec::new();
                for (ordinal, block) in blocks.into_iter().enumerate() {
                    match block {
                        ContentBlock::Text { text } => parts.push((ordinal, text)),
                        other @ (ContentBlock::Image { .. }
                        | ContentBlock::ToolUse { .. }
                        | ContentBlock::ToolResult { .. }
                        | ContentBlock::ServerToolUse { .. }
                        | ContentBlock::ToolSearchToolResult { .. }
                        | ContentBlock::WebSearchToolResult { .. }
                        | ContentBlock::WebFetchToolResult { .. }
                        | ContentBlock::CodeExecutionToolResult { .. }
                        | ContentBlock::BashCodeExecutionToolResult { .. }
                        | ContentBlock::TextEditorCodeExecutionToolResult { .. }
                        | ContentBlock::McpToolUse { .. }
                        | ContentBlock::McpToolResult { .. }) => {
                            tracing::debug!(message_id=%message.message_id,block=?std::mem::discriminant(&other),"scanner excludes non-prose assistant block");
                        }
                    }
                }
                (ScanRole::Assistant, false, parts)
            }
            Some(
                MessageContent::Tool(_)
                | MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_)
                | MessageContent::Skill(_),
            )
            | None => (ScanRole::Assistant, true, Vec::new()),
        };
        identities.push((
            message.message_id.clone(),
            message_seq,
            hash(encoded(&parts)?),
        ));
        for (part, text) in &parts {
            if text.is_empty() {
                continue;
            }
            let digest = hash(text.as_bytes());
            let offset = match start {
                ScanPosition::Fragment {
                    sequence_id,
                    message_id,
                    part_ordinal,
                    utf8_offset,
                    prose_digest,
                } if *sequence_id == message_seq => {
                    if message_id != &message.message_id {
                        return Err(invalid(
                            "scanner source position no longer identifies same message",
                        ));
                    }
                    if part < part_ordinal {
                        continue;
                    }
                    if part == part_ordinal {
                        if prose_digest != &digest || !text.is_char_boundary(*utf8_offset) {
                            return Err(invalid("scanner source fragment changed"));
                        }
                        *utf8_offset
                    } else {
                        0
                    }
                }
                ScanPosition::Boundary { .. } | ScanPosition::Fragment { .. } => 0,
            };
            if offset >= text.len() {
                continue;
            }
            let mut stop = match through {
                Some(ScanPosition::Fragment {
                    sequence_id,
                    message_id,
                    part_ordinal,
                    utf8_offset,
                    prose_digest,
                }) if *sequence_id == message_seq && part == part_ordinal => {
                    if message_id != &message.message_id
                        || prose_digest != &digest
                        || *utf8_offset > text.len()
                        || *utf8_offset < offset
                        || !text.is_char_boundary(*utf8_offset)
                    {
                        return Err(invalid("captured source end changed"));
                    }
                    *utf8_offset
                }
                None | Some(ScanPosition::Boundary { .. } | ScanPosition::Fragment { .. }) => {
                    text.len()
                }
            };
            let mut fragment;
            loop {
                let id = hash(encoded(&(
                    transcript,
                    &message.message_id,
                    generation,
                    part,
                    offset,
                    stop,
                    &digest,
                ))?);
                fragment = ScanFragment {
                    fragment_id: id,
                    message_id: message.message_id.clone(),
                    role,
                    origin: origin.clone(),
                    is_meta,
                    created_at_us: message.created_at_us,
                    part_ordinal: *part,
                    utf8_start: offset,
                    utf8_end: stop,
                    complete_message: offset == 0 && stop == text.len() && parts.len() == 1,
                    prose_digest: digest.clone(),
                    text: text
                        .get(offset..stop)
                        .ok_or_else(|| invalid("invalid UTF8 source fragment boundary"))?
                        .to_owned(),
                };
                let mut trial = fragments.clone();
                trial.push(fragment.clone());
                if encoded(&trial)?.len() <= 8192 {
                    break;
                }
                if !fragments.is_empty() {
                    return captured(fragments, end, &identities);
                }
                let remaining = stop.saturating_sub(offset);
                if remaining <= 1 {
                    return Err(invalid("scanner fragment metadata exceeds source budget"));
                }
                stop = offset + remaining / 2;
                while !text.is_char_boundary(stop) {
                    stop -= 1;
                }
            }
            end = ScanPosition::Fragment {
                sequence_id: message_seq,
                message_id: message.message_id.clone(),
                part_ordinal: *part,
                utf8_offset: stop,
                prose_digest: digest,
            };
            fragments.push(fragment);
            if through == Some(&end) {
                return captured(fragments, end, &identities);
            }
            if stop < text.len() {
                return captured(fragments, end, &identities);
            }
        }
        end = ScanPosition::Boundary {
            sequence_id: if lane == ScanLane::Live {
                message_seq
            } else {
                sqlx::query_scalar("SELECT COALESCE(MAX(sequence_id),0) FROM messages WHERE conversation_id=?1 AND sequence_id<?2").bind(transcript).bind(message_seq).fetch_one(&mut **tx).await?
            },
        };
    }
    captured(fragments, end, &identities)
}
fn captured(
    mut fragments: Vec<ScanFragment>,
    end: ScanPosition,
    identities: &[(String, i64, String)],
) -> DbResult<Captured> {
    fragments.sort_by_key(|f| {
        (
            identities
                .iter()
                .find(|(id, _, _)| id == &f.message_id)
                .map_or(0, |(_, seq, _)| *seq),
            f.part_ordinal,
            f.utf8_start,
        )
    });
    Ok(Captured {
        source_digest: hash(encoded(&(&fragments, &end))?),
        fragments,
        end,
    })
}
struct ProseProjection {
    message_id: String,
    sequence_id: i64,
    origin: InputOrigin,
    created_at_us: i64,
    content: Option<MessageContent>,
}
fn project_source(row: &sqlx::sqlite::SqliteRow) -> DbResult<ProseProjection> {
    let message_id: String = row.try_get("message_id")?;
    let message_type: String = row.try_get("message_type")?;
    if row.try_get::<bool, _>("has_attachments")? {
        tracing::debug!(%message_id,"scanner prose projection excludes attachment payloads");
    }
    let content = match message_type.as_str() {
        "user" | "agent" => {
            if row.try_get::<i64, _>("content_bytes")? > 1_048_576 {
                return Err(invalid(
                    "source row exceeds1MiB projection limit; remains explicitly pending",
                ));
            }
            let value = serde_json::from_str(&row.try_get::<String, _>("prose_content")?)
                .map_err(|e| invalid(format!("unreadable source JSON: {e}")))?;
            let kind = if message_type == "user" {
                crate::MessageType::User
            } else {
                crate::MessageType::Agent
            };
            Some(
                MessageContent::from_stored_json(kind, value)
                    .map_err(|e| invalid(format!("unreadable typed source: {e}")))?,
            )
        }
        "tool" | "system" | "error" | "continuation" | "skill" => {
            tracing::debug!(%message_id,%message_type,"scanner excludes non-discovery message payload");
            None
        }
        _ => return Err(invalid("unknown source message kind")),
    };
    Ok(ProseProjection {
        message_id,
        sequence_id: row.try_get("sequence_id")?,
        origin: crate::decode_origin(row)?,
        created_at_us: chrono::DateTime::parse_from_rfc3339(
            &row.try_get::<String, _>("created_at")?,
        )
        .map_err(|e| invalid(format!("unreadable source time: {e}")))?
        .timestamp_micros(),
        content,
    })
}

fn clip(value: &str, limit: usize) -> DbResult<(String, bool)> {
    let mut end = value.len();
    while encoded(
        value
            .get(..end)
            .ok_or_else(|| invalid("invalid UTF8 candidate clip boundary"))?,
    )?
    .len()
        > limit
    {
        end = end.saturating_sub(1);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
    }
    Ok((
        value
            .get(..end)
            .ok_or_else(|| invalid("invalid UTF8 candidate clip boundary"))?
            .to_owned(),
        end < value.len(),
    ))
}
pub(super) fn retired_candidate_state(
    disposition: TrackingRetirement,
    retired_at_us: i64,
    fragments: &[ScanFragment],
) -> ScanCandidateState {
    ScanCandidateState::Retired {
        disposition,
        later_source_fragment_ids: fragments
            .iter()
            .filter(|fragment| fragment.created_at_us > retired_at_us)
            .map(|fragment| fragment.fragment_id.clone())
            .collect(),
    }
}

pub(super) async fn candidates(
    tx: &mut Transaction<'_, Sqlite>,
    fragments: &[ScanFragment],
) -> DbResult<(Vec<ScanCandidate>, usize, usize)> {
    let ids: Vec<&str> = fragments.iter().map(|f| f.message_id.as_str()).collect();
    let ids = String::from_utf8(encoded(&ids)?).map_err(|e| invalid(e.to_string()))?;
    let rows=sqlx::query("SELECT i.id,i.kind,i.title,i.resumption,COALESCE(r.disposition,'tracking') AS disposition,r.reason,r.recorded_at_us AS retired_at_us FROM coordinator_tracking_items i LEFT JOIN coordinator_tracking_retirements r ON r.item_id=i.id AND r.item_revision=i.current_retirement_revision ORDER BY EXISTS(SELECT 1 FROM coordinator_tracking_evidence e WHERE e.item_id=i.id AND e.message_id IN(SELECT value FROM json_each(?1))) DESC,i.updated_at_us DESC,i.id LIMIT 16").bind(ids).fetch_all(&mut **tx).await?;
    let totals=sqlx::query("SELECT COUNT(*) AS all_count,COALESCE(SUM(current_retirement_revision IS NULL),0) AS active_count FROM coordinator_tracking_items").fetch_one(&mut **tx).await?;
    let active: usize = usize::try_from(totals.try_get::<i64, _>("active_count")?)
        .map_err(|e| invalid(e.to_string()))?;
    let all: usize = usize::try_from(totals.try_get::<i64, _>("all_count")?)
        .map_err(|e| invalid(e.to_string()))?;
    let mut out = Vec::new();
    let mut included_active = 0;
    for row in rows {
        let kind = match row.try_get::<String, _>("kind")?.as_str() {
            "requested_work" => TrackingKind::RequestedWork,
            "bookmark" => TrackingKind::Bookmark,
            _ => return Err(invalid("invalid tracking kind")),
        };
        let state = match row.try_get::<String, _>("disposition")?.as_str() {
            "tracking" => ScanCandidateState::Tracking,
            "resolved" => retired_candidate_state(
                TrackingRetirement::Resolved,
                row.try_get("retired_at_us")?,
                fragments,
            ),
            "dismissed" => retired_candidate_state(
                TrackingRetirement::Dismissed,
                row.try_get("retired_at_us")?,
                fragments,
            ),
            _ => return Err(invalid("invalid disposition")),
        };
        let is_tracking = matches!(&state, ScanCandidateState::Tracking);
        let (title, title_truncated) = clip(&row.try_get::<String, _>("title")?, 256)?;
        let hint = if is_tracking {
            row.try_get::<String, _>("resumption")?
        } else {
            row.try_get::<String, _>("reason")?
        };
        let (context_hint, context_hint_truncated) = clip(&hint, 256)?;
        let candidate = ScanCandidate {
            id: row.try_get("id")?,
            kind,
            state,
            title,
            title_truncated,
            context_hint,
            context_hint_truncated,
        };
        out.push(candidate);
        if encoded(&out)?.len() > 8192 {
            out.pop();
            break;
        }
        if is_tracking {
            included_active += 1;
        }
    }
    let included_retired = out.len() - included_active;
    Ok((
        out,
        active - included_active,
        all - active - included_retired,
    ))
}
