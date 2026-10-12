//! Persisted transcript reads and bounded render-unit-aligned selections.

const MAX_MESSAGE_HISTORY_LIMIT: i64 = 500;

use super::handlers::AppError;
use futures::future::BoxFuture;

pub(super) struct TranscriptObservations<'a> {
    db: &'a crate::db::Database,
}

impl<'a> TranscriptObservations<'a> {
    pub(super) fn new(db: &'a crate::db::Database) -> Self {
        Self { db }
    }

    pub(super) async fn latest(
        &self,
        id: &str,
        limit: i64,
    ) -> Result<StableTranscriptRead<(Vec<crate::db::Message>, bool, Option<i64>)>, AppError> {
        let limit = validate_message_history_limit("limit", Some(limit), limit)?;
        let db = self.db;
        let stable = stable_transcript_read(db, id, |db, id, _attempt| {
            Box::pin(async move {
                let (messages, has_older_messages) =
                    get_latest_aligned_messages(db, id, limit).await?;
                let server_message_tail = get_server_message_tail(db, id).await?;
                Ok((messages, has_older_messages, server_message_tail))
            })
        })
        .await?;
        Ok(stable)
    }

    pub(super) async fn before(
        &self,
        id: &str,
        before: i64,
        limit: i64,
    ) -> Result<StableTranscriptRead<(Vec<crate::db::Message>, bool, Option<i64>)>, AppError> {
        let limit = validate_message_history_limit("limit", Some(limit), limit)?;
        let db = self.db;
        let stable = stable_transcript_read(db, id, |db, id, _attempt| {
            Box::pin(async move {
                let (messages, has_older_messages) =
                    get_messages_before_aligned(db, id, before, limit).await?;
                let server_message_tail = get_server_message_tail(db, id).await?;
                Ok((messages, has_older_messages, server_message_tail))
            })
        })
        .await?;
        Ok(stable)
    }
}

const STABLE_TRANSCRIPT_READ_MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone)]
pub(super) struct StableTranscriptRead<T> {
    pub(super) conversation: crate::db::Conversation,
    pub(super) value: T,
    #[cfg(test)]
    pub(super) attempts: usize,
}

#[cfg(test)]
pub(super) type StableTranscriptReadTestHook = Box<dyn FnOnce(&str) -> bool>;

#[cfg(test)]
thread_local! {
    pub(super) static STABLE_TRANSCRIPT_READ_TEST_HOOK: std::cell::RefCell<Option<StableTranscriptReadTestHook>> =
        std::cell::RefCell::new(None);
}

pub(super) async fn stable_transcript_read<T, F>(
    db: &crate::db::Database,
    conversation_id: &str,
    mut read_value: F,
) -> Result<StableTranscriptRead<T>, AppError>
where
    F: for<'a> FnMut(&'a crate::db::Database, &'a str, usize) -> BoxFuture<'a, Result<T, AppError>>,
{
    for attempt in 1..=STABLE_TRANSCRIPT_READ_MAX_ATTEMPTS {
        let before = db
            .get_conversation(conversation_id)
            .await
            .map_err(|e| AppError::NotFound(e.to_string()))?;
        let value = read_value(db, conversation_id, attempt).await?;
        bump_transcript_generation_for_test(db, conversation_id).await?;
        let after = db
            .get_conversation(conversation_id)
            .await
            .map_err(|e| AppError::NotFound(e.to_string()))?;
        if before.transcript_generation == after.transcript_generation {
            return Ok(StableTranscriptRead {
                conversation: after,
                value,
                #[cfg(test)]
                attempts: attempt,
            });
        }
        tracing::debug!(
            conversation_id,
            attempt,
            before_generation = before.transcript_generation,
            after_generation = after.transcript_generation,
            "discarding transcript read because generation changed during response assembly"
        );
    }

    Err(AppError::Internal(format!(
        "conversation {conversation_id} transcript generation changed during {STABLE_TRANSCRIPT_READ_MAX_ATTEMPTS} read attempts"
    )))
}

#[cfg(test)]
async fn bump_transcript_generation_for_test(
    db: &crate::db::Database,
    conversation_id: &str,
) -> Result<(), AppError> {
    if STABLE_TRANSCRIPT_READ_TEST_HOOK
        .with(|hook| hook.borrow_mut().take())
        .is_some_and(|hook| hook(conversation_id))
    {
        sqlx::query(
            "UPDATE conversations
                 SET transcript_generation = transcript_generation + 1
                 WHERE id = ?1",
        )
        .bind(conversation_id)
        .execute(db.pool())
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    }
    Ok(())
}

#[cfg(not(test))]
#[allow(clippy::unused_async)]
async fn bump_transcript_generation_for_test(
    _db: &crate::db::Database,
    _conversation_id: &str,
) -> Result<(), AppError> {
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StreamDbMessageSelection {
    Latest,
    None,
}

type StreamTranscriptCoverage = crate::runtime::TranscriptCoverage;

pub(super) const STREAM_LAST_SEQUENCE_READ_FAILED: &str = "stream_last_sequence_read_failed";

pub(super) fn stream_last_sequence_read_failed(
    conversation_id: &str,
    error: impl std::fmt::Display,
) -> AppError {
    AppError::TypedInternal {
        message: format!(
            "failed to read last sequence id for conversation {conversation_id}: {error}"
        ),
        error_type: STREAM_LAST_SEQUENCE_READ_FAILED.to_string(),
    }
}

pub(super) async fn get_stream_last_sequence_id(
    db: &crate::db::Database,
    conversation_id: &str,
) -> Result<i64, AppError> {
    db.get_last_sequence_id(conversation_id)
        .await
        .map_err(|e| stream_last_sequence_read_failed(conversation_id, e))
}

fn message_is_non_empty_system(message: &crate::db::Message) -> bool {
    matches!(
        (&message.message_type, &message.content),
        (crate::db::MessageType::System, crate::db::MessageContent::System(content))
            if !content.text.trim().is_empty()
    )
}

pub(super) fn stable_render_unit_start_index(
    messages: &[crate::db::Message],
    target_sequence: i64,
    reached_transcript_start: bool,
) -> Option<usize> {
    let target_index = messages
        .iter()
        .position(|message| message.sequence_id == target_sequence)?;
    let target = &messages[target_index];
    if matches!(
        target.message_type,
        crate::db::MessageType::User | crate::db::MessageType::Skill
    ) {
        return Some(target_index);
    }

    let reset_index = messages[..target_index].iter().rposition(|message| {
        matches!(
            message.message_type,
            crate::db::MessageType::User | crate::db::MessageType::Skill
        )
    });
    if reset_index.is_none() && !reached_transcript_start {
        return None;
    }
    let segment_start = reset_index.map_or(0, |index| index + 1);
    let segment = &messages[segment_start..=target_index];
    if let Some(agent_offset) = segment
        .iter()
        .position(|message| matches!(message.message_type, crate::db::MessageType::Agent))
    {
        return Some(segment_start + agent_offset);
    }
    if let Some(system_offset) = segment.iter().position(message_is_non_empty_system) {
        return Some(segment_start + system_offset);
    }
    reset_index.or(Some(segment_start))
}

const RENDER_UNIT_BACKFILL_CHUNK_SIZE: i64 = 64;

fn render_unit_alignment_ceiling_error() -> AppError {
    AppError::TypedBadRequest {
        message: format!(
            "Aligned message slice exceeds the server response ceiling of {MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES} messages"
        ),
        error_type: "message_slice_render_unit_ceiling_exceeded".to_string(),
    }
}

async fn align_slice_start_to_render_unit(
    db: &crate::db::Database,
    conversation_id: &str,
    messages: &mut Vec<crate::db::Message>,
) -> Result<(), AppError> {
    let Some(first) = messages.first() else {
        return Ok(());
    };
    let target_sequence = first.sequence_id;
    if matches!(
        first.message_type,
        crate::db::MessageType::User | crate::db::MessageType::Skill
    ) {
        return Ok(());
    }

    if messages.len() > MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES {
        return Err(render_unit_alignment_ceiling_error());
    }

    let mut before_sequence = first.sequence_id;
    let mut intervening = Vec::new();
    let mut remaining_prefix_budget = MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES - messages.len();
    while remaining_prefix_budget > 0 {
        let fetch_limit = RENDER_UNIT_BACKFILL_CHUNK_SIZE.min(
            i64::try_from(remaining_prefix_budget)
                .map_err(|_| AppError::Internal("render-unit ceiling exceeds i64".to_string()))?,
        );
        let previous = db
            .get_messages_before(conversation_id, before_sequence, fetch_limit)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        #[cfg(test)]
        tests::record_alignment_prefix_read(conversation_id, before_sequence, &previous);
        if previous.is_empty() {
            intervening.append(messages);
            let boundary_index =
                stable_render_unit_start_index(&intervening, target_sequence, true).unwrap_or(0);
            *messages = intervening.split_off(boundary_index);
            return Ok(());
        }
        remaining_prefix_budget -= previous.len();
        let oldest_sequence = previous[0].sequence_id;

        let mut candidate = previous;
        candidate.append(&mut intervening);
        candidate.append(messages);

        if let Some(boundary_index) =
            stable_render_unit_start_index(&candidate, target_sequence, false)
        {
            let aligned = candidate.split_off(boundary_index);
            *messages = aligned;
            return Ok(());
        }

        let mut older_intervening = candidate;
        older_intervening.append(&mut intervening);
        intervening = older_intervening;
        before_sequence = oldest_sequence;
    }

    let has_uncollected_older = if intervening.is_empty() {
        has_messages_before(db, conversation_id, messages).await?
    } else {
        has_messages_before(db, conversation_id, &intervening).await?
    };
    if has_uncollected_older {
        return Err(render_unit_alignment_ceiling_error());
    }

    intervening.append(messages);
    let boundary_index =
        stable_render_unit_start_index(&intervening, target_sequence, true).unwrap_or(0);
    *messages = intervening.split_off(boundary_index);
    Ok(())
}

async fn has_messages_before(
    db: &crate::db::Database,
    conversation_id: &str,
    messages: &[crate::db::Message],
) -> Result<bool, AppError> {
    let Some(first) = messages.first() else {
        return Ok(false);
    };
    Ok(!db
        .get_messages_before(conversation_id, first.sequence_id, 1)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .is_empty())
}

async fn get_latest_aligned_messages(
    db: &crate::db::Database,
    conversation_id: &str,
    limit: i64,
) -> Result<(Vec<crate::db::Message>, bool), AppError> {
    let requested_count = usize::try_from(limit)
        .map_err(|_| AppError::BadRequest("limit is too large for this server".to_string()))?;
    let mut messages = db
        .get_latest_messages(conversation_id, limit + 1)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if messages.len() > requested_count {
        messages.remove(0);
    }
    #[cfg(test)]
    tests::reset_before_alignment(db, conversation_id, &messages).await?;
    align_slice_start_to_render_unit(db, conversation_id, &mut messages).await?;
    let has_older_messages = has_messages_before(db, conversation_id, &messages).await?;
    Ok((messages, has_older_messages))
}

async fn get_messages_before_aligned(
    db: &crate::db::Database,
    conversation_id: &str,
    before: i64,
    limit: i64,
) -> Result<(Vec<crate::db::Message>, bool), AppError> {
    let requested_count = usize::try_from(limit)
        .map_err(|_| AppError::BadRequest("limit is too large for this server".to_string()))?;
    let mut messages = db
        .get_messages_before(conversation_id, before, limit + 1)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if messages.len() > requested_count {
        messages.remove(0);
    }
    #[cfg(test)]
    tests::reset_before_alignment(db, conversation_id, &messages).await?;
    align_slice_start_to_render_unit(db, conversation_id, &mut messages).await?;
    let has_older_messages = has_messages_before(db, conversation_id, &messages).await?;
    Ok((messages, has_older_messages))
}

pub(super) async fn get_server_message_tail(
    db: &crate::db::Database,
    conversation_id: &str,
) -> Result<Option<i64>, AppError> {
    let tail = db
        .get_last_sequence_id(conversation_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok((tail > 0).then_some(tail))
}

pub(super) const DEFAULT_MESSAGE_HISTORY_LIMIT: i64 = 50;
pub(super) const MAX_RENDER_UNIT_ALIGNED_RESPONSE_MESSAGES: usize = 2_048;
pub(super) fn db_message_selection_for_stream(
    cursor_replay_served: bool,
    after_event_sequence: Option<i64>,
    client_transcript_generation: Option<i64>,
    last_sequence_id: i64,
    transcript_generation: i64,
) -> StreamDbMessageSelection {
    if client_transcript_generation
        .is_some_and(|query_generation| query_generation != transcript_generation)
    {
        return StreamDbMessageSelection::Latest;
    }

    if cursor_replay_served
        && client_transcript_generation == Some(transcript_generation)
        && after_event_sequence
            .is_some_and(|after_event_sequence| after_event_sequence >= last_sequence_id)
    {
        return StreamDbMessageSelection::None;
    }

    StreamDbMessageSelection::Latest
}

pub(super) fn latest_stream_transcript_coverage(
    messages: &[crate::db::Message],
    server_message_tail: Option<i64>,
    has_older_messages: bool,
) -> StreamTranscriptCoverage {
    let loaded_tail = messages.last().map(|message| message.sequence_id);
    if has_older_messages || loaded_tail != server_message_tail {
        crate::runtime::TranscriptCoverage::Tail
    } else {
        crate::runtime::TranscriptCoverage::Complete
    }
}

pub(super) fn preserved_stream_transcript_coverage() -> StreamTranscriptCoverage {
    crate::runtime::TranscriptCoverage::Preserve
}

pub(super) async fn read_stream_init_messages_with_tail(
    db: &crate::db::Database,
    conversation_id: &str,
    after_event_sequence: Option<i64>,
    client_transcript_generation: Option<i64>,
    cursor_replay_served: bool,
    attempt: usize,
    last_sequence_id: Result<i64, AppError>,
) -> Result<
    (
        i64,
        StreamDbMessageSelection,
        Vec<crate::db::Message>,
        Option<i64>,
        StreamTranscriptCoverage,
    ),
    AppError,
> {
    let last_sequence_id = last_sequence_id?;
    let mut db_message_selection = db_message_selection_for_stream(
        cursor_replay_served,
        after_event_sequence,
        client_transcript_generation,
        last_sequence_id,
        db.get_conversation(conversation_id)
            .await
            .map_err(|e| AppError::NotFound(e.to_string()))?
            .transcript_generation,
    );
    if attempt > 1 {
        db_message_selection = StreamDbMessageSelection::Latest;
    }
    let server_message_tail = get_server_message_tail(db, conversation_id).await?;
    let (messages, transcript_coverage) = match db_message_selection {
        StreamDbMessageSelection::None => (Vec::new(), preserved_stream_transcript_coverage()),
        StreamDbMessageSelection::Latest => {
            match get_latest_aligned_messages(db, conversation_id, DEFAULT_MESSAGE_HISTORY_LIMIT)
                .await
            {
                Ok((messages, has_older_messages)) => {
                    let coverage = latest_stream_transcript_coverage(
                        &messages,
                        server_message_tail,
                        has_older_messages,
                    );
                    (messages, coverage)
                }
                Err(AppError::TypedBadRequest { error_type, .. })
                    if error_type == "message_slice_render_unit_ceiling_exceeded" =>
                {
                    tracing::warn!(
                        conv_id = %conversation_id,
                        limit = DEFAULT_MESSAGE_HISTORY_LIMIT,
                        "Using bounded unaligned SSE init because the current render unit exceeds the response ceiling"
                    );
                    let messages = db
                        .get_latest_messages(conversation_id, DEFAULT_MESSAGE_HISTORY_LIMIT)
                        .await
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                    let coverage =
                        latest_stream_transcript_coverage(&messages, server_message_tail, true);
                    (messages, coverage)
                }
                Err(error) => return Err(error),
            }
        }
    };
    Ok((
        last_sequence_id,
        db_message_selection,
        messages,
        server_message_tail,
        transcript_coverage,
    ))
}
pub(super) fn validate_message_history_limit(
    name: &str,
    value: Option<i64>,
    default: i64,
) -> Result<i64, AppError> {
    let value = value.unwrap_or(default);
    if value <= 0 {
        return Err(AppError::BadRequest(format!(
            "{name} must be greater than 0"
        )));
    }
    Ok(value.min(MAX_MESSAGE_HISTORY_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, Message, MessageContent};
    use phoenix_core::domain::llm_types::ContentBlock;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };

    type AlignmentResetAction = Box<
        dyn for<'a> FnOnce(
                &'a Database,
                &'a str,
                &'a [Message],
            ) -> BoxFuture<'a, Result<(), AppError>>
            + Send,
    >;
    type PrefixReads = Arc<Mutex<Vec<(i64, Vec<String>)>>>;

    struct AlignmentResetHook {
        conversation_id: String,
        reset: Option<AlignmentResetAction>,
        prefix_reads: PrefixReads,
    }

    thread_local! {
        static ALIGNMENT_RESET_HOOK: std::cell::RefCell<Option<AlignmentResetHook>> =
            const { std::cell::RefCell::new(None) };
    }

    pub(super) async fn reset_before_alignment(
        db: &Database,
        conversation_id: &str,
        messages: &[Message],
    ) -> Result<(), AppError> {
        let reset = ALIGNMENT_RESET_HOOK.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .filter(|hook| hook.conversation_id == conversation_id)
                .and_then(|hook| hook.reset.take())
        });
        if let Some(reset) = reset {
            reset(db, conversation_id, messages).await?;
        }
        Ok(())
    }

    pub(super) fn record_alignment_prefix_read(
        conversation_id: &str,
        before_sequence: i64,
        messages: &[Message],
    ) {
        let prefix_reads = ALIGNMENT_RESET_HOOK.with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|hook| hook.conversation_id == conversation_id)
                .map(|hook| Arc::clone(&hook.prefix_reads))
        });
        if let Some(prefix_reads) = prefix_reads {
            prefix_reads
                .lock()
                .expect("prefix oracle lock")
                .push((before_sequence, message_ids(messages)));
        }
    }

    struct AlignmentHookGuard;

    impl Drop for AlignmentHookGuard {
        fn drop(&mut self) {
            ALIGNMENT_RESET_HOOK.with(|slot| *slot.borrow_mut() = None);
        }
    }

    fn message_ids(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .map(|message| message.message_id.clone())
            .collect()
    }

    async fn replace_history(
        db: &Database,
        conversation_id: &str,
        label: &str,
        bump_generation: bool,
    ) -> Result<(), sqlx::Error> {
        let mut tx = db.pool().begin().await?;
        sqlx::query("DELETE FROM messages WHERE conversation_id = ?1")
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        let contents = [
            MessageContent::user(format!("{label} older")),
            MessageContent::user(format!("{label} prompt")),
            MessageContent::agent(vec![ContentBlock::ToolUse {
                id: format!("{label}-call"),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": label}),
            }]),
            MessageContent::tool(format!("{label}-call"), format!("{label} result 1"), false),
            MessageContent::tool(format!("{label}-call"), format!("{label} result 2"), false),
            MessageContent::tool(format!("{label}-call"), format!("{label} tail"), false),
        ];
        for (sequence, content) in (1_i64..).zip(contents) {
            sqlx::query(
                "INSERT INTO messages (
                    message_id, conversation_id, sequence_id, message_type,
                    content, display_data, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(format!("{label}-{sequence}"))
            .bind(conversation_id)
            .bind(sequence)
            .bind(content.message_type().to_string())
            .bind(content.to_stored_json().to_string())
            .bind(serde_json::json!({"history": label, "sequence": sequence}).to_string())
            .bind("2026-10-12T00:00:00Z")
            .execute(&mut *tx)
            .await?;
        }
        if bump_generation {
            sqlx::query(
                "UPDATE conversations SET transcript_generation = transcript_generation + 1
                 WHERE id = ?1",
            )
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    #[allow(clippy::too_many_lines)]
    async fn assert_causal_alignment_reset(before: Option<i64>) {
        let db = Database::open_in_memory()
            .await
            .expect("disposable database");
        let conversation_id = if before.is_some() {
            "alignment-before"
        } else {
            "alignment-latest"
        };
        let initial = db
            .create_conversation(conversation_id, conversation_id, "/tmp", true, None, None)
            .await
            .expect("create conversation");
        replace_history(&db, conversation_id, "old", false)
            .await
            .expect("old history");
        let committed = Arc::new(AtomicBool::new(false));
        let prefix_reads = Arc::new(Mutex::new(Vec::new()));
        let committed_by_hook = Arc::clone(&committed);
        let reset: AlignmentResetAction = Box::new(move |db, id, selected| {
            assert_eq!(
                message_ids(selected),
                if before.is_some() {
                    vec!["old-4", "old-5"]
                } else {
                    vec!["old-5", "old-6"]
                },
                "hook must run after lookahead trimming and before backfill",
            );
            Box::pin(async move {
                replace_history(db, id, "new", true)
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?;
                committed_by_hook.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        ALIGNMENT_RESET_HOOK.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(AlignmentResetHook {
                conversation_id: conversation_id.to_string(),
                reset: Some(reset),
                prefix_reads: Arc::clone(&prefix_reads),
            });
        });
        let guard = AlignmentHookGuard;
        reset_before_alignment(&db, "unrelated-conversation", &[])
            .await
            .expect("unrelated read must not consume reset");
        assert!(!committed.load(Ordering::SeqCst));

        let observations = TranscriptObservations::new(&db);
        let result = match before {
            Some(cursor) => observations.before(conversation_id, cursor, 2).await,
            None => observations.latest(conversation_id, 2).await,
        }
        .expect("stable read across committed alignment reset");
        assert!(committed.load(Ordering::SeqCst));
        ALIGNMENT_RESET_HOOK.with(|slot| {
            assert!(slot.borrow().as_ref().expect("armed hook").reset.is_none());
        });
        assert_eq!(result.attempts, 2);
        assert_eq!(
            result.conversation.transcript_generation,
            initial.transcript_generation + 1
        );
        let expected_prefix = if before.is_some() {
            (
                4,
                vec![
                    "new-1".to_string(),
                    "new-2".to_string(),
                    "new-3".to_string(),
                ],
            )
        } else {
            (
                5,
                vec![
                    "new-1".to_string(),
                    "new-2".to_string(),
                    "new-3".to_string(),
                    "new-4".to_string(),
                ],
            )
        };
        assert_eq!(
            *prefix_reads.lock().expect("prefix oracle lock"),
            vec![expected_prefix.clone(), expected_prefix],
            "both the discarded attempt and retry must actually backfill from replacement rows",
        );
        drop(guard);

        let fresh = match before {
            Some(cursor) => observations.before(conversation_id, cursor, 2).await,
            None => observations.latest(conversation_id, 2).await,
        }
        .expect("fresh post-reset read");
        assert_eq!(fresh.attempts, 1);
        assert_eq!(
            fresh.conversation.transcript_generation,
            result.conversation.transcript_generation
        );
        assert_eq!(
            serde_json::to_value(&result.value).unwrap(),
            serde_json::to_value(&fresh.value).unwrap()
        );
        let (messages, has_older, tail) = result.value;
        assert_eq!(
            message_ids(&messages),
            if before.is_some() {
                vec!["new-3", "new-4", "new-5"]
            } else {
                vec!["new-3", "new-4", "new-5", "new-6"]
            },
        );
        assert_eq!(messages.first().map(|message| message.sequence_id), Some(3));
        assert_eq!(
            messages.first().map(|message| message.message_type),
            Some(crate::db::MessageType::Agent)
        );
        assert!(has_older);
        assert_eq!(tail, Some(6));
        if let Some(cursor) = before {
            assert!(messages.iter().all(|message| message.sequence_id < cursor));
        }
        let replacement = db
            .get_messages(conversation_id)
            .await
            .expect("complete replacement history");
        let expected_end = if before.is_some() { 5 } else { 6 };
        assert_eq!(
            serde_json::to_value(&messages).unwrap(),
            serde_json::to_value(&replacement[2..expected_end]).unwrap()
        );
        db.pool().close().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn latest_causal_alignment_reset_retries_committed_history() {
        assert_causal_alignment_reset(None).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn before_causal_alignment_reset_retries_committed_history() {
        assert_causal_alignment_reset(Some(6)).await;
    }
}
