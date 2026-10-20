//! JSON-reference aliases for reads whose opaque identities cannot occupy URL path segments.

#[cfg(test)]
mod tests;

use super::{
    get_conversation, get_conversation_message_range, get_conversation_messages,
    get_conversation_messages_around, get_conversation_messages_latest, get_product_conversation,
    resolve_coordinator_route, AppError, AroundMessagesQuery, CoordinatorRouteResponse,
    GetConversationQuery, LatestMessagesQuery, MessageHistoryQuery, MessageRangeQuery,
};
use crate::api::product_conversations::{
    get_coordinator_history, CoordinatorHistoryQuery, OrdinarySnapshotResponse, SnapshotQuery,
};
use crate::api::types::{
    ConversationMessageRangeResponse, ConversationMessageSliceResponse,
    ConversationMessagesAroundResponse, ConversationWithMessagesResponse,
};
use crate::api::AppState;
use axum::{
    extract::{Path, Query, State},
    routing::post,
    Json, Router,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactReferenceRequest {
    reference: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/conversation-reads/snapshot", post(snapshot))
        .route("/api/conversation-reads/messages/latest", post(latest))
        .route("/api/conversation-reads/messages", post(messages))
        .route("/api/conversation-reads/messages/range", post(range))
        .route(
            "/api/conversation-reads/messages/around/:sequence",
            post(around),
        )
        .route(
            "/api/product-conversation-reads/snapshot",
            post(product_snapshot),
        )
        .route(
            "/api/global/coordinator/reads/route",
            post(coordinator_route),
        )
        .route(
            "/api/global/coordinator/reads/history",
            post(coordinator_history),
        )
}

async fn snapshot(
    state: State<AppState>,
    query: Query<GetConversationQuery>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<ConversationWithMessagesResponse>, AppError> {
    get_conversation(state, Path(request.reference), query).await
}

async fn latest(
    state: State<AppState>,
    query: Query<LatestMessagesQuery>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<ConversationMessageSliceResponse>, AppError> {
    get_conversation_messages_latest(state, Path(request.reference), query).await
}

async fn messages(
    state: State<AppState>,
    query: Query<MessageHistoryQuery>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<ConversationMessageSliceResponse>, AppError> {
    get_conversation_messages(state, Path(request.reference), query).await
}

async fn range(
    state: State<AppState>,
    query: Query<MessageRangeQuery>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<ConversationMessageRangeResponse>, AppError> {
    get_conversation_message_range(state, Path(request.reference), query).await
}

async fn around(
    state: State<AppState>,
    Path(sequence): Path<i64>,
    query: Query<AroundMessagesQuery>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<ConversationMessagesAroundResponse>, AppError> {
    get_conversation_messages_around(state, Path((request.reference, sequence)), query).await
}

async fn product_snapshot(
    state: State<AppState>,
    query: Query<SnapshotQuery>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<OrdinarySnapshotResponse>, AppError> {
    get_product_conversation(state, Path(request.reference), query).await
}

async fn coordinator_route(
    state: State<AppState>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<CoordinatorRouteResponse>, AppError> {
    resolve_coordinator_route(state, Path(request.reference)).await
}

async fn coordinator_history(
    state: State<AppState>,
    query: Query<CoordinatorHistoryQuery>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<crate::api::types::CoordinatorConversationSnapshotView>, AppError> {
    get_coordinator_history(state, Path(request.reference), query).await
}
