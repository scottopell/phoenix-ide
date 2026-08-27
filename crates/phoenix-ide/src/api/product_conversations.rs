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
    ProductConversationCloseInspectionView, ProductConversationCloseLossView,
    ProductConversationClosePhaseView, ProductConversationCloseView,
    ProductConversationHandoffView, ProductConversationListResponse, ProductConversationListRow,
    ProductConversationPresentationView, ProductConversationSegmentView,
    ProductConversationSnapshotView, ProductConversationSourceRelationView,
    ProductConversationSourceView, ProductConversationTranscriptRowView,
    ProductConversationWorkIdentityView,
};
use super::AppState;
use crate::db::{
    DbError, ProductConversationAggregate, ProductConversationHandoff,
    ProductConversationListProjection, ProductConversationSegment,
    ProductConversationSegmentCeiling, ProductConversationSource, ProductConversationSourceKind,
};
use crate::send_chat_service::accepts_user_message_direct_or_steering;

const DEFAULT_MESSAGE_LIMIT: usize = 50;
const MAX_MESSAGE_LIMIT: usize = 200;
const PRODUCT_CONVERSATION_ROUTE_PREFIX: &str = "/product-conversations/";

#[derive(Debug, Deserialize)]
pub struct SnapshotQuery {
    pub before: Option<String>,
    pub message_limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AggregateCursor {
    product_conversation_id: String,
    generation: Vec<AggregateGeneration>,
    segment_ceilings: Vec<AggregateSegmentCeiling>,
    before_segment_ordinal: i64,
    before_sequence_id: i64,
    before_message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AggregateGeneration {
    transcript_row_id: String,
    transcript_generation: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
struct AggregateSegmentCeiling {
    transcript_row_id: String,
    tail_sequence_id: i64,
    tail_message_id: Option<String>,
}

pub async fn list_product_conversations(
    State(state): State<AppState>,
) -> Result<Json<ProductConversationListResponse>, AppError> {
    let rows = state
        .db
        .list_ordinary_product_conversation_projections()
        .await
        .map_err(db_to_app)?
        .iter()
        .map(list_row)
        .collect();
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
    let cursor = query.before.as_deref().map(decode_cursor).transpose()?;
    let segment_ceilings = cursor.as_ref().map(|cursor| {
        cursor
            .segment_ceilings
            .iter()
            .map(|ceiling| ProductConversationSegmentCeiling {
                transcript_row_id: ceiling.transcript_row_id.clone(),
                tail_sequence_id: ceiling.tail_sequence_id,
                tail_message_id: ceiling.tail_message_id.clone(),
            })
            .collect::<Vec<_>>()
    });
    let snapshot = state
        .db
        .read_ordinary_product_conversation_snapshot(
            &reference,
            cursor.as_ref().map(|cursor| {
                (
                    cursor.before_segment_ordinal,
                    cursor.before_sequence_id,
                    cursor.before_message_id.clone(),
                )
            }),
            segment_ceilings.as_deref(),
            message_limit + 1,
        )
        .await
        .map_err(db_to_app)?;
    if let Some(cursor) = &cursor {
        validate_cursor(&snapshot.aggregate, cursor)?;
    }
    Ok(Json(
        snapshot_view(
            &state,
            snapshot.aggregate,
            snapshot.requested_transcript_row_id,
            cursor,
            message_limit,
            snapshot.messages,
        )
        .await?,
    ))
}

fn canonical_route(aggregate: &ProductConversationAggregate) -> String {
    format!(
        "{PRODUCT_CONVERSATION_ROUTE_PREFIX}{}",
        aggregate.product_conversation.id()
    )
}

fn list_row(projection: &ProductConversationListProjection) -> ProductConversationListRow {
    ProductConversationListRow {
        product_conversation_id: projection.product_conversation_id.to_string(),
        canonical_route: format!(
            "{PRODUCT_CONVERSATION_ROUTE_PREFIX}{}",
            projection.product_conversation_id
        ),
        canonical_root: ProductConversationTranscriptRowView {
            transcript_row_id: projection.root_transcript_row_id.clone(),
            slug: projection.root_slug.clone(),
            title: projection.root_title.clone(),
        },
        ordinary_lifecycle: lifecycle_view(projection.lifecycle),
        latest_transcript_row_id: projection.latest_transcript_row_id.clone(),
        updated_at: projection.updated_at.to_rfc3339(),
        presentation: presentation(
            projection.root_title.as_deref(),
            projection.root_slug.as_deref(),
            &projection.latest_state,
            projection.latest_continued_in_conv_id.is_some(),
            &projection.latest_transcript_row_id,
        ),
    }
}

async fn snapshot_view(
    state: &AppState,
    mut aggregate: ProductConversationAggregate,
    requested_transcript_row_id: String,
    cursor: Option<AggregateCursor>,
    message_limit: usize,
    page: Vec<(i64, crate::db::Message)>,
) -> Result<ProductConversationSnapshotView, AppError> {
    let lifecycle = lifecycle_from_archived(
        aggregate
            .segments
            .last()
            .expect("aggregate has segment")
            .transcript_row
            .conversation
            .archived,
    );
    let close_obligation = state
        .db
        .get_active_close_obligation_for_product(aggregate.product_conversation.id())
        .await
        .map_err(db_to_app)?;
    let close = match close_obligation.as_ref() {
        Some(obligation) => Some(close_view(&state.db, obligation).await?),
        None => None,
    };
    let generation = aggregate_generation(&aggregate);
    let segment_ceilings = cursor.as_ref().map_or_else(
        || aggregate_segment_ceilings(&aggregate),
        |cursor| cursor.segment_ceilings.clone(),
    );
    let (messages, has_older, page_messages, boundary_message_ids) =
        aggregate_messages(&aggregate, page, message_limit);
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
        close,

        requested_transcript_row_id,
        canonical_root: transcript_row_view(&aggregate.root),
        ordinary_lifecycle: lifecycle_view(lifecycle),
        latest_transcript_row_id: latest_id.clone(),
        writable_transcript_row_id: writable_transcript_row_id(state, lifecycle, &aggregate).await,
        updated_at: aggregate.updated_at.to_rfc3339(),
        presentation: presentation(
            aggregate.root.conversation.title.as_deref(),
            aggregate.root.conversation.slug.as_deref(),
            &aggregate
                .segments
                .last()
                .expect("aggregate has segment")
                .transcript_row
                .conversation
                .state,
            aggregate
                .segments
                .last()
                .expect("aggregate has segment")
                .transcript_row
                .conversation
                .continued_in_conv_id
                .is_some(),
            &latest_id,
        ),
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
                &segment_ceilings,
                &page_messages,
            ))
        }),
        has_older,
    })
}

type AggregatePageMessages =
    std::collections::HashMap<String, Vec<crate::api::wire::EnrichedMessage>>;
type AggregatePage = (
    AggregatePageMessages,
    bool,
    Vec<(i64, crate::db::Message)>,
    std::collections::HashSet<String>,
);

fn aggregate_messages(
    aggregate: &ProductConversationAggregate,
    mut messages: Vec<(i64, crate::db::Message)>,
    limit: usize,
) -> AggregatePage {
    let has_older = messages.len() > limit;
    messages.truncate(limit);
    let boundary_message_ids = aggregate
        .segments
        .iter()
        .filter_map(|segment| segment.handoff.as_ref())
        .flat_map(|handoff| {
            std::iter::once(handoff.continuation_message_id()).chain(
                handoff
                    .accepted_is_duplicate_summary()
                    .then(|| handoff.accepted_successor_message_id())
                    .flatten(),
            )
        })
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
        segment_messages.sort_by(|left, right| {
            left.sequence_id
                .cmp(&right.sequence_id)
                .then_with(|| left.message_id.cmp(&right.message_id))
        });
    }
    let page_boundary_message_ids = messages
        .iter()
        .map(|(_, message)| message.message_id.clone())
        .filter(|message_id| boundary_message_ids.contains(message_id.as_str()))
        .collect();
    (by_segment, has_older, messages, page_boundary_message_ids)
}

fn next_cursor(
    aggregate: &ProductConversationAggregate,
    generation: &[AggregateGeneration],
    segment_ceilings: &[AggregateSegmentCeiling],
    page_messages: &[(i64, crate::db::Message)],
) -> AggregateCursor {
    let (segment_ordinal, message) = page_messages
        .last()
        .expect("cursor emitted only for a non-empty page");
    let sequence_id = message.sequence_id;
    let message_id = message.message_id.clone();
    AggregateCursor {
        product_conversation_id: aggregate.product_conversation.id().to_string(),
        generation: generation.to_vec(),
        segment_ceilings: segment_ceilings.to_vec(),
        before_segment_ordinal: *segment_ordinal,
        before_sequence_id: sequence_id,
        before_message_id: message_id,
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
            .filter(|handoff| page_boundary_message_ids.contains(handoff.continuation_message_id()))
            .map(handoff_view),
    }
}

fn handoff_view(handoff: &ProductConversationHandoff) -> ProductConversationHandoffView {
    match handoff {
        ProductConversationHandoff::Completed {
            predecessor_transcript_row_id,
            successor_transcript_row_id,
            continuation_message_id,
            accepted_successor_message_id,
            summary,
            ..
        } => ProductConversationHandoffView::Completed {
            predecessor_transcript_row_id: predecessor_transcript_row_id.clone(),
            successor_transcript_row_id: successor_transcript_row_id.clone(),
            continuation_message_id: continuation_message_id.clone(),
            accepted_successor_message_id: accepted_successor_message_id.clone(),
            summary: summary.clone(),
        },
        ProductConversationHandoff::Historical {
            predecessor_transcript_row_id,
            successor_transcript_row_id,
            continuation_message_id,
            summary,
        } => ProductConversationHandoffView::Historical {
            predecessor_transcript_row_id: predecessor_transcript_row_id.clone(),
            successor_transcript_row_id: successor_transcript_row_id.clone(),
            continuation_message_id: continuation_message_id.clone(),
            summary: summary.clone(),
        },
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

fn presentation(
    root_title: Option<&str>,
    root_slug: Option<&str>,
    latest_state: &crate::state_machine::ConvState,
    latest_has_successor: bool,
    fallback_id: &str,
) -> ProductConversationPresentationView {
    let display_name = root_title.or(root_slug).unwrap_or(fallback_id).to_string();
    if matches!(
        latest_state,
        crate::state_machine::ConvState::ContextExhausted { .. }
    ) && !latest_has_successor
    {
        ProductConversationPresentationView::NeedsAction { display_name }
    } else {
        let presentation_mode = if matches!(
            latest_state,
            crate::state_machine::ConvState::ContextExhausted { .. }
        ) {
            "done"
        } else {
            latest_state.presentation_mode()
        };
        ProductConversationPresentationView::State {
            display_name,
            presentation_mode: presentation_mode.to_string(),
        }
    }
}

fn work_identity(
    aggregate: &ProductConversationAggregate,
) -> Option<ProductConversationWorkIdentityView> {
    let row = aggregate
        .segments
        .iter()
        .rev()
        .find(|segment| segment.transcript_row.work_identity.is_some())?;
    let identity = row.transcript_row.work_identity.as_ref()?;
    Some(ProductConversationWorkIdentityView {
        work_transcript_row_id: row.transcript_row.conversation.id.clone(),
        worktree_path: identity.worktree_path.clone(),
        branch_name: identity.branch_name.clone(),
        base_branch: identity.base_branch.clone(),
        task_id: identity.task_id.clone(),
        task_title: identity.task_title.clone(),
    })
}

fn lifecycle_from_archived(archived: bool) -> OrdinaryProductConversationLifecycle {
    if archived {
        OrdinaryProductConversationLifecycle::History
    } else {
        OrdinaryProductConversationLifecycle::Open
    }
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

fn aggregate_segment_ceilings(
    aggregate: &ProductConversationAggregate,
) -> Vec<AggregateSegmentCeiling> {
    aggregate
        .segments
        .iter()
        .map(|segment| AggregateSegmentCeiling {
            transcript_row_id: segment.transcript_row.conversation.id.clone(),
            tail_sequence_id: segment.transcript_row.tail_sequence_id,
            tail_message_id: segment.transcript_row.tail_message_id.clone(),
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
    accepts_user_message_direct_or_steering(&effective_state).then_some(latest.id)
}

fn validate_cursor(
    aggregate: &ProductConversationAggregate,
    cursor: &AggregateCursor,
) -> Result<(), AppError> {
    if cursor.product_conversation_id != aggregate.product_conversation.id().as_str()
        || cursor.generation != aggregate_generation(aggregate)
    {
        return Err(AppError::BadRequest(
            "stale or mismatched product conversation cursor".to_string(),
        ));
    }
    Ok(())
}

async fn close_view(
    db: &crate::db::Database,
    obligation: &phoenix_core::domain::close::CloseObligation,
) -> Result<ProductConversationCloseView, AppError> {
    use phoenix_core::domain::close::ClosePhase;

    let phase = match obligation.phase() {
        ClosePhase::AwaitingBlockerResolution => {
            ProductConversationClosePhaseView::AwaitingBlockerResolution
        }
        ClosePhase::AwaitingStopWorkConfirmation => {
            ProductConversationClosePhaseView::AwaitingStopWorkConfirmation
        }
        ClosePhase::SettlingActiveWork => ProductConversationClosePhaseView::SettlingActiveWork,
        ClosePhase::CancelRequestedDuringSettlement => {
            ProductConversationClosePhaseView::CancelRequestedDuringSettlement
        }
        ClosePhase::AwaitingRetirementInspection => {
            ProductConversationClosePhaseView::AwaitingRetirementInspection
        }
        ClosePhase::AwaitingLossConfirmation => {
            ProductConversationClosePhaseView::AwaitingLossConfirmation
        }
        ClosePhase::RetirementRequested => ProductConversationClosePhaseView::RetirementRequested,
        ClosePhase::NeedsRepair => ProductConversationClosePhaseView::NeedsRepair,
        ClosePhase::Completed => ProductConversationClosePhaseView::Completed,
    };
    Ok(ProductConversationCloseView {
        attempt_id: obligation.attempt_id().to_string(),
        phase,
        inspections: db
            .list_close_retirement_inspections(obligation.attempt_id().as_str())
            .await
            .map_err(db_to_app)?
            .into_iter()
            .map(|inspection| ProductConversationCloseInspectionView {
                scope: inspection.target.scope.as_str().to_string(),
                generation: inspection.snapshot.generation().to_string(),
                fingerprint: inspection.snapshot.fingerprint().to_string(),
            })
            .collect(),
        losses: db
            .list_close_retirement_losses(obligation.attempt_id().as_str())
            .await
            .map_err(db_to_app)?
            .into_iter()
            .map(|loss| ProductConversationCloseLossView {
                scope: loss.scope.as_str().to_string(),
                generation: loss.snapshot.generation().to_string(),
                category: format!("{:?}", loss.item),
                identity: format!("{:?}", loss.item),
            })
            .collect(),
    })
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

    async fn create_completed_continuation(
        state: &AppState,
        root: &crate::db::Conversation,
        boundary_message_id: &str,
        opening_message_id: &str,
    ) -> crate::db::Conversation {
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
        let (outcome, _) = state
            .db
            .continue_conversation_with_intent(
                &root.id,
                crate::db::NewContinuationDispatchIntent {
                    message_id: opening_message_id.to_string(),
                    handoff: "accepted opening handoff".to_string(),
                    user_agent: None,
                },
            )
            .await
            .unwrap();
        let successor = match outcome {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected continuation, got {other:?}")
            }
        };
        state
            .db
            .add_message(
                boundary_message_id,
                &root.id,
                &MessageContent::Continuation(ContinuationContent {
                    summary: "exact persisted handoff".to_string(),
                }),
                None,
                None,
            )
            .await
            .unwrap();
        state
            .db
            .add_message(
                opening_message_id,
                &successor.id,
                &MessageContent::user("exact persisted handoff"),
                None,
                None,
            )
            .await
            .unwrap();
        successor
    }

    #[test]
    fn rejects_stale_or_cross_aggregate_cursor() {
        let cursor = AggregateCursor {
            product_conversation_id: "one".to_string(),
            generation: vec![],
            segment_ceilings: vec![],
            before_segment_ordinal: 0,
            before_sequence_id: 1,
            before_message_id: "message-1".to_string(),
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
        let successor = create_completed_continuation(&state, &root, "handoff", "opening").await;

        state
            .db
            .begin_close_foundation(&root.product_conversation_id, "snapshot-close")
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
            format!("/product-conversations/{}", root.product_conversation_id)
        );
        assert_eq!(rows[0]["presentation"]["display_name"], "Root Slug");
        assert_eq!(rows[0]["latest_transcript_row_id"], successor.id);

        let snapshot = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?message_limit=2",
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
            format!("/product-conversations/{}", root.product_conversation_id)
        );
        assert_eq!(snapshot["segments"][0]["segment_ordinal"], 0);
        assert_eq!(snapshot["segments"][0]["handoff"]["kind"], "completed");
        assert_eq!(
            snapshot["segments"][0]["handoff"]["continuation_message_id"],
            "handoff"
        );
        assert_eq!(
            snapshot["segments"][0]["handoff"]["accepted_successor_message_id"],
            "opening"
        );
        assert!(snapshot["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|segment| segment["messages"].as_array().unwrap())
            .all(
                |message| message["message_id"] != "handoff" && message["message_id"] != "opening"
            ));
        assert_eq!(snapshot["writable_transcript_row_id"], successor.id);
        assert_eq!(snapshot["close"]["attempt_id"], "snapshot-close");
        assert_eq!(snapshot["close"]["phase"], "awaiting_blocker_resolution");
    }

    #[tokio::test]
    async fn edited_completed_opening_remains_an_ordinary_message() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        create_completed_continuation(&state, &root, "handoff", "opening").await;
        sqlx::query("UPDATE messages SET content = ? WHERE message_id = 'opening'")
            .bind(
                serde_json::to_string(&MessageContent::user("edited opening").to_stored_json())
                    .unwrap(),
            )
            .execute(state.db.pool())
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
        assert_eq!(snapshot["segments"][0]["handoff"]["kind"], "completed");
        assert!(snapshot["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|segment| segment["messages"].as_array().unwrap())
            .any(|message| message["message_id"] == "opening"));
        assert!(snapshot["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|segment| segment["messages"].as_array().unwrap())
            .all(|message| message["message_id"] != "handoff"));
    }

    #[tokio::test]
    async fn historical_edge_renders_boundary_without_successor_message_id() {
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
                "legacy-handoff",
                &root.id,
                &MessageContent::Continuation(ContinuationContent {
                    summary: "legacy persisted handoff".to_string(),
                }),
                None,
                None,
            )
            .await
            .unwrap();
        state
            .db
            .add_message(
                "legacy-opening",
                &successor.id,
                &MessageContent::user("legacy opening"),
                None,
                None,
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
        let handoff = &snapshot["segments"][0]["handoff"];
        assert_eq!(handoff["kind"], "historical");
        assert_eq!(handoff["continuation_message_id"], "legacy-handoff");
        assert!(handoff.get("accepted_successor_message_id").is_none());
        assert!(snapshot["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|segment| segment["messages"].as_array().unwrap())
            .all(|message| message["message_id"] != "legacy-handoff"));
        assert!(snapshot["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|segment| segment["messages"].as_array().unwrap())
            .any(|message| message["message_id"] == "legacy-opening"));
    }

    #[tokio::test]
    async fn aggregate_identity_uses_root_title_while_presentation_uses_latest_state() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        let successor = create_completed_continuation(&state, &root, "handoff", "opening").await;
        let response = create_router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/product-conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let row = &body["product_conversations"][0];
        assert_eq!(row["canonical_root"]["title"], "Root Slug");
        assert_eq!(row["presentation"]["kind"], "state");
        assert_eq!(row["presentation"]["display_name"], "Root Slug");
        assert_eq!(row["presentation"]["presentation_mode"], "idle");
        assert_eq!(row["latest_transcript_row_id"], successor.id);
    }

    #[test]
    fn presentation_is_a_structurally_exclusive_action_or_state_value() {
        let action = presentation(
            Some("Root"),
            None,
            &ConvState::ContextExhausted {
                summary: "full".to_string(),
            },
            false,
            "fallback",
        );
        let continued = presentation(
            Some("Root"),
            None,
            &ConvState::ContextExhausted {
                summary: "full".to_string(),
            },
            true,
            "fallback",
        );
        assert!(matches!(
            action,
            ProductConversationPresentationView::NeedsAction { .. }
        ));
        assert!(matches!(
            continued,
            ProductConversationPresentationView::State { presentation_mode, .. }
                if presentation_mode == "done"
        ));
    }

    #[tokio::test]
    async fn canonical_route_is_product_keyed_and_aliases_survive_root_rename() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        let expected_route = format!("/product-conversations/{}", root.product_conversation_id);
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

    #[tokio::test]
    async fn lifecycle_uses_shipped_archived_authority_not_dormant_product_lifecycle() {
        let state = make_test_state().await;
        let conversation = state
            .db
            .create_conversation("archived", "archived", "/tmp", true, None, None)
            .await
            .unwrap();
        sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?")
            .bind(conversation.product_conversation_id.as_str())
            .execute(state.db.pool())
            .await
            .unwrap();

        let response = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/api/product-conversations/{}", conversation.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let open: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(open["ordinary_lifecycle"], "open");

        state
            .db
            .archive_conversation(&conversation.id)
            .await
            .unwrap();
        sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'open' WHERE id = ?")
            .bind(conversation.product_conversation_id.as_str())
            .execute(state.db.pool())
            .await
            .unwrap();
        let response = create_router(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/product-conversations/{}", conversation.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let archived: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(archived["ordinary_lifecycle"], "history");
    }

    #[tokio::test]
    async fn work_identity_uses_attached_environment_and_preserves_typed_task_summary() {
        let state = make_test_state().await;
        let conversation = state
            .db
            .create_conversation("work", "work", "/tmp", true, None, None)
            .await
            .unwrap();
        let scope_id = conversation.attached_work_scope_id.as_ref().unwrap();
        sqlx::query(
            "UPDATE work_scopes
             SET environment_kind = 'allocated_worktree', cwd = '/tmp/worktree',
                 worktree_path = '/tmp/worktree', branch_name = 'feature/work', base_branch = 'main'
             WHERE id = ?",
        )
        .bind(scope_id.as_str())
        .execute(state.db.pool())
        .await
        .unwrap();
        sqlx::query(
            "UPDATE conversations
             SET cm_kind = 'direct', cm_task_id = '17002', cm_task_title = 'Persist product conversation'
             WHERE id = ?",
        )
        .bind(&conversation.id)
        .execute(state.db.pool())
        .await
        .unwrap();

        let response = create_router(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/product-conversations/{}", conversation.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let snapshot: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(snapshot["work_identity"]["worktree_path"], "/tmp/worktree");
        assert_eq!(snapshot["work_identity"]["branch_name"], "feature/work");
        assert_eq!(snapshot["work_identity"]["base_branch"], "main");
        assert_eq!(snapshot["work_identity"]["task_id"], "17002");
        assert_eq!(
            snapshot["work_identity"]["task_title"],
            "Persist product conversation"
        );
    }

    #[tokio::test]
    async fn writable_target_accepts_latest_segment_busy_or_cancelling_steering_states() {
        let state = make_test_state().await;
        for (slug, state_value) in [
            ("busy", ConvState::LlmRequesting { attempt: 1 }),
            (
                "cancelling",
                ConvState::CancellingSubAgents {
                    pending: vec![],
                    completed_results: vec![],
                    cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
                    spawn_tool_id: None,
                },
            ),
        ] {
            let conversation = state
                .db
                .create_conversation(slug, slug, "/tmp", true, None, None)
                .await
                .unwrap();
            state
                .db
                .update_conversation_state(&conversation.id, &state_value)
                .await
                .unwrap();
            let response = create_router(state.clone())
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/product-conversations/{}", conversation.id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let snapshot: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(snapshot["writable_transcript_row_id"], conversation.id);
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn aggregate_cursor_pages_without_gaps_or_duplicates_and_allows_tail_append() {
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
        let successor =
            create_completed_continuation(&state, &root, "root-handoff", "opening").await;
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
        let cursor_after_append = emitted_cursor.expect("first page emits a cursor");
        let appended_response = create_router(state)
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?before={cursor_after_append}",
                        root.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(appended_response.status(), StatusCode::OK);
        let appended: serde_json::Value = serde_json::from_slice(
            &to_bytes(appended_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let appended_ids = appended["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|segment| segment["messages"].as_array().unwrap())
            .map(|message| message["message_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(appended_ids, vec!["root-1"]);
    }

    #[tokio::test]
    async fn aggregate_cursor_rejects_topology_generation_change() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        for message_id in ["one", "two"] {
            state
                .db
                .add_message(
                    message_id,
                    &root.id,
                    &MessageContent::user(message_id),
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        let first = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?message_limit=1",
                        root.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(first.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let cursor = first["before"].as_str().unwrap();
        sqlx::query("UPDATE conversations SET transcript_generation = transcript_generation + 1 WHERE id = ?1")
            .bind(&root.id)
            .execute(state.db.pool())
            .await
            .unwrap();
        let response = create_router(state)
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?before={cursor}",
                        root.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn aggregate_cursor_uses_message_id_to_resume_duplicate_sequence_ids() {
        let state = make_test_state().await;
        let root = state
            .db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        for message_id in ["duplicate-a", "duplicate-b"] {
            state
                .db
                .add_message(
                    message_id,
                    &root.id,
                    &MessageContent::user(message_id),
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        sqlx::query("UPDATE messages SET sequence_id = 1 WHERE conversation_id = ?")
            .bind(&root.id)
            .execute(state.db.pool())
            .await
            .unwrap();

        let first = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?message_limit=1",
                        root.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(first.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let cursor = first["before"].as_str().unwrap().to_string();
        let second = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?before={cursor}",
                        root.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second: serde_json::Value =
            serde_json::from_slice(&to_bytes(second.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let first_id = first["segments"][0]["messages"][0]["message_id"]
            .as_str()
            .unwrap();
        let second_id = second["segments"][0]["messages"][0]["message_id"]
            .as_str()
            .unwrap();
        assert_ne!(first_id, second_id);

        let ordered = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?message_limit=2",
                        root.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let ordered: serde_json::Value =
            serde_json::from_slice(&to_bytes(ordered.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let ordered_ids = ordered["segments"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["message_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ordered_ids, vec!["duplicate-a", "duplicate-b"]);

        state
            .db
            .add_message(
                "duplicate-z",
                &root.id,
                &MessageContent::user("duplicate-z"),
                None,
                None,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE messages SET sequence_id = 1 WHERE message_id = 'duplicate-z'")
            .execute(state.db.pool())
            .await
            .unwrap();
        let appended_response = create_router(state)
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/product-conversations/{}?before={cursor}",
                        root.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(appended_response.status(), StatusCode::OK);
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
