use axum::{
    extract::{Path, Query, State},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use phoenix_core::domain::product_conversation::OrdinaryProductConversationLifecycle;
use serde::{Deserialize, Serialize};

use super::handlers::AppError;
use super::types::{
    OrdinaryProductConversationLifecycleView, ProductConversationChainQaCompatibilityView,
    ProductConversationHandoffView, ProductConversationListResponse, ProductConversationListRow,
    ProductConversationPresentationView, ProductConversationSegmentView,
    ProductConversationSnapshotView, ProductConversationSourceRelationView,
    ProductConversationSourceView, ProductConversationTranscriptRowView,
    ProductConversationWorkIdentityView,
};
use super::AppState;
use crate::db::{
    DbError, ProductConversationAggregate, ProductConversationSegment, ProductConversationSource,
    ProductConversationSourceKind,
};

const DEFAULT_MESSAGE_LIMIT: usize = 50;
const MAX_MESSAGE_LIMIT: usize = 200;

#[derive(Debug, Deserialize)]
pub struct SnapshotQuery {
    pub before: Option<String>,
    pub message_limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AggregateCursor {
    product_conversation_id: String,
    generation: Vec<AggregateGeneration>,
    tail_watermarks: Vec<AggregateTailWatermark>,
    before_segment_ordinal: i64,
    before_sequence_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AggregateGeneration {
    transcript_row_id: String,
    transcript_generation: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AggregateTailWatermark {
    transcript_row_id: String,
    tail_sequence_id: i64,
}

pub async fn list_product_conversations(
    State(state): State<AppState>,
) -> Result<Json<ProductConversationListResponse>, AppError> {
    let aggregates = state
        .db
        .list_ordinary_product_conversations()
        .await
        .map_err(db_to_app)?;
    let rows = aggregates
        .iter()
        .map(list_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(ProductConversationListResponse {
        product_conversations: rows,
    }))
}

pub async fn get_product_conversation(
    State(state): State<AppState>,
    Path(reference): Path<String>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<ProductConversationSnapshotView>, AppError> {
    let message_limit = message_limit(query.message_limit)?;
    let resolved = state
        .db
        .resolve_ordinary_product_conversation(&reference)
        .await
        .map_err(db_to_app)?;
    let aggregate = state
        .db
        .get_ordinary_product_conversation(&resolved.product_conversation_id)
        .await
        .map_err(db_to_app)?;
    let cursor = query.before.as_deref().map(decode_cursor).transpose()?;
    if let Some(cursor) = &cursor {
        validate_cursor(&aggregate, cursor)?;
    }
    Ok(Json(
        snapshot_view(
            &state,
            aggregate,
            resolved.requested_transcript_row_id,
            cursor,
            message_limit,
        )
        .await?,
    ))
}

fn canonical_route(aggregate: &ProductConversationAggregate) -> String {
    format!("/c/{}", aggregate.product_conversation.id())
}

fn list_row(
    aggregate: &ProductConversationAggregate,
) -> Result<ProductConversationListRow, AppError> {
    let lifecycle = lifecycle_view(
        aggregate
            .product_conversation
            .ordinary_lifecycle()
            .ok_or_else(|| AppError::Internal("ordinary aggregate lost lifecycle".to_string()))?,
    );
    Ok(ProductConversationListRow {
        product_conversation_id: aggregate.product_conversation.id().to_string(),
        canonical_route: canonical_route(aggregate),
        canonical_root: transcript_row_view(&aggregate.root),
        ordinary_lifecycle: lifecycle,
        root_transcript_row_id: aggregate.root.conversation.id.clone(),
        latest_transcript_row_id: aggregate.latest_transcript_row_id.clone(),
        updated_at: aggregate.updated_at.to_rfc3339(),
        presentation: presentation(&aggregate.root.conversation),
    })
}

async fn snapshot_view(
    state: &AppState,
    mut aggregate: ProductConversationAggregate,
    requested_transcript_row_id: String,
    cursor: Option<AggregateCursor>,
    message_limit: usize,
) -> Result<ProductConversationSnapshotView, AppError> {
    let lifecycle = aggregate
        .product_conversation
        .ordinary_lifecycle()
        .ok_or_else(|| AppError::Internal("ordinary aggregate lost lifecycle".to_string()))?;
    let generation = aggregate_generation(&aggregate);
    let tail_watermarks = aggregate_tail_watermarks(&aggregate);
    let (messages, has_older, page_messages, boundary_message_ids) =
        aggregate_messages(&state.db, &aggregate, cursor.as_ref(), message_limit).await?;
    let source = match aggregate.source.as_mut() {
        Some(source) => {
            source.deleted = state
                .db
                .source_conversation_is_deleted(source)
                .await
                .map_err(db_to_app)?;
            Some(source_view(source))
        }
        None => None,
    };
    let root_id = aggregate.root.conversation.id.clone();
    let latest_id = aggregate.latest_transcript_row_id.clone();
    let segments = aggregate
        .segments
        .iter()
        .map(|segment| segment_view(segment, &messages, &boundary_message_ids))
        .collect();
    Ok(ProductConversationSnapshotView {
        product_conversation_id: aggregate.product_conversation.id().to_string(),
        canonical_route: canonical_route(&aggregate),
        requested_transcript_row_id,
        canonical_root: transcript_row_view(&aggregate.root),
        ordinary_lifecycle: lifecycle_view(lifecycle),
        root_transcript_row_id: root_id.clone(),
        latest_transcript_row_id: latest_id.clone(),
        writable_transcript_row_id: writable_transcript_row_id(state, lifecycle, &aggregate).await,
        updated_at: aggregate.updated_at.to_rfc3339(),
        presentation: presentation(&aggregate.root.conversation),
        work_identity: work_identity(&aggregate),
        source,
        chain_qa_compatibility: (aggregate.segments.len() > 1).then(|| {
            ProductConversationChainQaCompatibilityView {
                url: format!("/api/chains/{root_id}"),
                root_transcript_row_id: root_id,
            }
        }),
        segments,
        before: has_older.then(|| {
            encode_cursor(&next_cursor(
                &aggregate,
                &generation,
                &tail_watermarks,
                &page_messages,
            ))
        }),
        has_older,
    })
}

async fn aggregate_messages(
    db: &crate::db::Database,
    aggregate: &ProductConversationAggregate,
    cursor: Option<&AggregateCursor>,
    limit: usize,
) -> Result<
    (
        std::collections::HashMap<String, Vec<crate::api::wire::EnrichedMessage>>,
        bool,
        Vec<(i64, crate::db::Message)>,
        std::collections::HashSet<String>,
    ),
    AppError,
> {
    let mut messages = db
        .get_product_conversation_messages_page(
            aggregate.product_conversation.id(),
            cursor.map(|cursor| (cursor.before_segment_ordinal, cursor.before_sequence_id)),
            limit + 1,
        )
        .await
        .map_err(db_to_app)?;
    let has_older = messages.len() > limit;
    messages.truncate(limit);
    let boundary_message_ids = aggregate
        .segments
        .iter()
        .filter_map(|segment| segment.handoff.as_ref())
        .map(|handoff| handoff.continuation_message_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut by_segment = std::collections::HashMap::<String, Vec<_>>::new();
    for (ordinal, message) in &messages {
        let _ = ordinal;
        if !boundary_message_ids.contains(message.message_id.as_str()) {
            by_segment
                .entry(message.conversation_id.clone())
                .or_default()
                .push(crate::api::wire::EnrichedMessage::from(message));
        }
    }
    for segment_messages in by_segment.values_mut() {
        segment_messages.sort_by_key(|message| message.sequence_id);
    }
    let page_boundary_message_ids = messages
        .iter()
        .map(|(_, message)| message.message_id.clone())
        .filter(|message_id| boundary_message_ids.contains(message_id.as_str()))
        .collect();
    Ok((by_segment, has_older, messages, page_boundary_message_ids))
}

fn next_cursor(
    aggregate: &ProductConversationAggregate,
    generation: &[AggregateGeneration],
    tail_watermarks: &[AggregateTailWatermark],
    page_messages: &[(i64, crate::db::Message)],
) -> AggregateCursor {
    let (segment_ordinal, message) = page_messages
        .last()
        .expect("cursor emitted only for a non-empty page");
    let sequence_id = message.sequence_id;
    AggregateCursor {
        product_conversation_id: aggregate.product_conversation.id().to_string(),
        generation: generation.to_vec(),
        tail_watermarks: tail_watermarks.to_vec(),
        before_segment_ordinal: *segment_ordinal,
        before_sequence_id: sequence_id,
    }
}

fn segment_view(
    segment: &ProductConversationSegment,
    messages: &std::collections::HashMap<String, Vec<crate::api::wire::EnrichedMessage>>,
    page_boundary_message_ids: &std::collections::HashSet<String>,
) -> ProductConversationSegmentView {
    ProductConversationSegmentView {
        segment_ordinal: segment.transcript_row.segment_ordinal,
        transcript_row_id: segment.transcript_row.conversation.id.clone(),
        slug: segment.transcript_row.conversation.slug.clone(),
        title: segment.transcript_row.conversation.title.clone(),
        messages: messages
            .get(&segment.transcript_row.conversation.id)
            .cloned()
            .unwrap_or_default(),
        handoff: segment
            .handoff
            .as_ref()
            .filter(|handoff| page_boundary_message_ids.contains(&handoff.continuation_message_id))
            .map(|handoff| ProductConversationHandoffView {
                predecessor_transcript_row_id: handoff.predecessor_transcript_row_id.clone(),
                successor_transcript_row_id: handoff.successor_transcript_row_id.clone(),
                continuation_message_id: handoff.continuation_message_id.clone(),
                summary: handoff.summary.clone(),
            }),
    }
}

fn transcript_row_view(
    row: &crate::db::ProductConversationTranscriptRow,
) -> ProductConversationTranscriptRowView {
    ProductConversationTranscriptRowView {
        transcript_row_id: row.conversation.id.clone(),
        slug: row.conversation.slug.clone(),
        title: row.conversation.title.clone(),
    }
}

fn presentation(conversation: &crate::db::Conversation) -> ProductConversationPresentationView {
    let presentation_mode = if matches!(
        conversation.state,
        crate::state_machine::ConvState::ContextExhausted { .. }
    ) {
        if conversation.continued_in_conv_id.is_some() {
            "done"
        } else {
            "needs_action"
        }
    } else {
        conversation.state.presentation_mode()
    };
    ProductConversationPresentationView {
        display_name: conversation
            .chain_name
            .clone()
            .or_else(|| conversation.title.clone())
            .or_else(|| conversation.slug.clone())
            .unwrap_or_else(|| conversation.id.clone()),
        presentation_mode: presentation_mode.to_string(),
        requires_action: presentation_mode == "needs_action",
        archived: conversation.archived,
    }
}

fn work_identity(
    aggregate: &ProductConversationAggregate,
) -> Option<ProductConversationWorkIdentityView> {
    let row = aggregate.segments.iter().rev().find(|segment| {
        segment
            .transcript_row
            .conversation
            .conv_mode
            .branch_name()
            .is_some()
    })?;
    let mode = &row.transcript_row.conversation.conv_mode;
    Some(ProductConversationWorkIdentityView {
        work_transcript_row_id: row.transcript_row.conversation.id.clone(),
        worktree_path: mode.worktree_path()?.to_string(),
        branch_name: mode.branch_name()?.to_string(),
        base_branch: mode.base_branch()?.to_string(),
        task_id: mode.task_id().map(str::to_string),
        task_title: mode.task_title().map(str::to_string),
    })
}

fn source_view(source: &ProductConversationSource) -> ProductConversationSourceView {
    let relation = match source.relation_kind {
        ProductConversationSourceKind::ApprovedTask => {
            ProductConversationSourceRelationView::ApprovedTask
        }
    };
    if source.deleted {
        ProductConversationSourceView::Deleted {
            source_product_conversation_id: source.source_product_conversation_id.to_string(),
            source_conversation_id: source.source_conversation_id.clone(),
            relation,
            relation_key: source.relation_key.clone(),
        }
    } else {
        ProductConversationSourceView::Present {
            source_product_conversation_id: source.source_product_conversation_id.to_string(),
            source_conversation_id: source.source_conversation_id.clone(),
            relation,
            relation_key: source.relation_key.clone(),
        }
    }
}

fn aggregate_generation(aggregate: &ProductConversationAggregate) -> Vec<AggregateGeneration> {
    aggregate
        .segments
        .iter()
        .map(|segment| AggregateGeneration {
            transcript_row_id: segment.transcript_row.conversation.id.clone(),
            transcript_generation: segment.transcript_row.conversation.transcript_generation,
        })
        .collect()
}

fn aggregate_tail_watermarks(
    aggregate: &ProductConversationAggregate,
) -> Vec<AggregateTailWatermark> {
    aggregate
        .segments
        .iter()
        .map(|segment| AggregateTailWatermark {
            transcript_row_id: segment.transcript_row.conversation.id.clone(),
            tail_sequence_id: segment.transcript_row.tail_sequence_id,
        })
        .collect()
}

async fn writable_transcript_row_id(
    state: &AppState,
    lifecycle: OrdinaryProductConversationLifecycle,
    aggregate: &ProductConversationAggregate,
) -> Option<String> {
    if lifecycle != OrdinaryProductConversationLifecycle::Open {
        return None;
    }
    let latest = aggregate
        .segments
        .last()?
        .transcript_row
        .conversation
        .clone();
    let effective_state = state
        .runtime
        .effective_conversation_state(&latest.id)
        .await
        .unwrap_or(latest.state);
    crate::state_machine::check_user_message_acceptable(&effective_state)
        .is_ok()
        .then_some(latest.id)
}

fn validate_cursor(
    aggregate: &ProductConversationAggregate,
    cursor: &AggregateCursor,
) -> Result<(), AppError> {
    if cursor.product_conversation_id != aggregate.product_conversation.id().as_str()
        || cursor.generation != aggregate_generation(aggregate)
        || cursor.tail_watermarks != aggregate_tail_watermarks(aggregate)
    {
        return Err(AppError::BadRequest(
            "stale or mismatched product conversation cursor".to_string(),
        ));
    }
    Ok(())
}

fn lifecycle_view(
    lifecycle: OrdinaryProductConversationLifecycle,
) -> OrdinaryProductConversationLifecycleView {
    match lifecycle {
        OrdinaryProductConversationLifecycle::Open => {
            OrdinaryProductConversationLifecycleView::Open
        }
        OrdinaryProductConversationLifecycle::History => {
            OrdinaryProductConversationLifecycleView::History
        }
    }
}

fn message_limit(value: Option<usize>) -> Result<usize, AppError> {
    let value = value.unwrap_or(DEFAULT_MESSAGE_LIMIT);
    if value == 0 || value > MAX_MESSAGE_LIMIT {
        return Err(AppError::BadRequest(format!(
            "message_limit must be between 1 and {MAX_MESSAGE_LIMIT}"
        )));
    }
    Ok(value)
}

fn encode_cursor(cursor: &AggregateCursor) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor).expect("cursor schema serializes"))
}

fn decode_cursor(cursor: &str) -> Result<AggregateCursor, AppError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| AppError::BadRequest("invalid product conversation cursor".to_string()))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid product conversation cursor".to_string()))
}

#[allow(clippy::wildcard_enum_match_arm)]
fn db_to_app(error: DbError) -> AppError {
    match error {
        DbError::ConversationNotFound(id) => AppError::NotFound(id),
        error => AppError::Internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::api::handlers::{create_router, hard_delete_cascade_tests::make_test_state};
    use crate::db::{ContinuationContent, ContinueOutcome, ConvState, MessageContent};

    #[test]
    fn rejects_stale_or_cross_aggregate_cursor() {
        let cursor = AggregateCursor {
            product_conversation_id: "one".to_string(),
            generation: vec![],
            tail_watermarks: vec![],
            before_segment_ordinal: 0,
            before_sequence_id: 1,
        };
        let encoded = encode_cursor(&cursor);
        assert_eq!(
            decode_cursor(&encoded).unwrap().product_conversation_id,
            "one"
        );
        assert!(decode_cursor("not a cursor").is_err());
    }

    #[tokio::test]
    async fn router_lists_one_ordinary_row_and_resolves_member_snapshot() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        state
            .db
            .update_conversation_state(
                &root.id,
                &ConvState::ContextExhausted {
                    summary: "exhausted".to_string(),
                },
            )
            .await
            .unwrap();
        let successor = match state.db.continue_conversation(&root.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected continuation, got {other:?}")
            }
        };
        state
            .db
            .add_message(
                "handoff",
                &root.id,
                &MessageContent::Continuation(ContinuationContent {
                    summary: "exact persisted handoff".to_string(),
                }),
                None,
                None,
            )
            .await
            .unwrap();

        let list = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/product-conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let listed: serde_json::Value =
            serde_json::from_slice(&to_bytes(list.into_body(), usize::MAX).await.unwrap()).unwrap();
        let rows = listed["product_conversations"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["canonical_root"]["slug"], "root-slug");
        assert_eq!(
            rows[0]["canonical_route"],
            format!("/c/{}", root.product_conversation_id)
        );
        assert_eq!(rows[0]["presentation"]["display_name"], "Root Slug");
        assert_eq!(rows[0]["latest_transcript_row_id"], successor.id);

        let snapshot = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?message_limit=1",
                        successor.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot.status(), StatusCode::OK);
        let snapshot: serde_json::Value =
            serde_json::from_slice(&to_bytes(snapshot.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(snapshot["requested_transcript_row_id"], successor.id);
        assert_eq!(
            snapshot["canonical_route"],
            format!("/c/{}", root.product_conversation_id)
        );
        assert_eq!(snapshot["segments"][0]["segment_ordinal"], 0);
        assert_eq!(
            snapshot["segments"][0]["handoff"]["continuation_message_id"],
            "handoff"
        );
        assert_eq!(snapshot["writable_transcript_row_id"], successor.id);
    }

    #[tokio::test]
    async fn canonical_route_is_product_keyed_and_aliases_survive_root_rename() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        let expected_route = format!("/c/{}", root.product_conversation_id);
        state
            .db
            .rename_conversation(&root.id, "renamed-root")
            .await
            .unwrap();

        for reference in [
            root.product_conversation_id.to_string(),
            root.id.clone(),
            "renamed-root".to_string(),
        ] {
            let response = create_router(state.clone())
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/product-conversations/{reference}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let snapshot: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(snapshot["canonical_route"], expected_route);
        }
    }

    #[tokio::test]
    async fn one_row_aggregate_has_no_chain_qa_compatibility_or_writable_exhausted_leaf() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        state
            .db
            .update_conversation_state(
                &root.id,
                &ConvState::ContextExhausted {
                    summary: "exhausted".to_string(),
                },
            )
            .await
            .unwrap();
        let response = create_router(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/product-conversations/{}", root.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert!(snapshot["chain_qa_compatibility"].is_null());
        assert!(snapshot["writable_transcript_row_id"].is_null());
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn aggregate_cursor_pages_without_gaps_or_duplicates_and_rejects_generation_replay() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        state
            .db
            .add_message(
                "root-1",
                &root.id,
                &MessageContent::user("root 1"),
                None,
                None,
            )
            .await
            .unwrap();
        state
            .db
            .update_conversation_state(
                &root.id,
                &ConvState::ContextExhausted {
                    summary: "exhausted".to_string(),
                },
            )
            .await
            .unwrap();
        let successor = match state.db.continue_conversation(&root.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected continuation, got {other:?}")
            }
        };
        state
            .db
            .add_message(
                "root-handoff",
                &root.id,
                &MessageContent::Continuation(ContinuationContent {
                    summary: "handoff".to_string(),
                }),
                None,
                None,
            )
            .await
            .unwrap();
        state
            .db
            .add_message(
                "successor-1",
                &successor.id,
                &MessageContent::user("successor 1"),
                None,
                None,
            )
            .await
            .unwrap();
        state
            .db
            .add_message(
                "successor-2",
                &successor.id,
                &MessageContent::user("successor 2"),
                None,
                None,
            )
            .await
            .unwrap();

        let mut before = None;
        let mut emitted_cursor = None;
        let mut seen = Vec::new();
        let mut seen_handoffs = Vec::new();
        loop {
            let suffix = before
                .as_ref()
                .map_or_else(String::new, |cursor| format!("&before={cursor}"));
            let response = create_router(state.clone())
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "/api/product-conversations/{}?message_limit=2{suffix}",
                            root.id
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let page: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            for segment in page["segments"].as_array().unwrap() {
                for message in segment["messages"].as_array().unwrap() {
                    seen.push(message["message_id"].as_str().unwrap().to_string());
                }
                if let Some(handoff) = segment["handoff"].as_object() {
                    seen_handoffs.push(
                        handoff["continuation_message_id"]
                            .as_str()
                            .unwrap()
                            .to_string(),
                    );
                }
            }
            before = page["before"].as_str().map(str::to_string);
            emitted_cursor.get_or_insert_with(|| before.clone().unwrap());
            if !page["has_older"].as_bool().unwrap() {
                break;
            }
        }
        seen.sort();
        assert_eq!(seen, vec!["root-1", "successor-1", "successor-2"]);
        assert_eq!(seen_handoffs, vec!["root-handoff"]);

        state
            .db
            .add_message(
                "later",
                &successor.id,
                &MessageContent::user("later"),
                None,
                None,
            )
            .await
            .unwrap();
        let stale_cursor = emitted_cursor.expect("first page emits a cursor");
        let stale_response = create_router(state)
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?before={stale_cursor}",
                        root.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale_response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn router_returns_404_only_for_excluded_references() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        let subagent = state
            .db
            .create_subagent_conversation(
                "sub",
                "sub",
                "/tmp",
                &root.id,
                "model",
                &root.conv_mode,
                phoenix_core::llm_language::LlmLanguage::default(),
                root.attached_work_scope_id.as_ref(),
            )
            .await
            .unwrap();
        let coordinator = state
            .db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let absent = "absent".to_string();
        for reference in [&subagent.id, &coordinator.id, &absent] {
            let response = create_router(state.clone())
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/product-conversations/{reference}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }
}
