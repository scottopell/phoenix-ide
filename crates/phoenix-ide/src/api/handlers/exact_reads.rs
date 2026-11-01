//! Read aliases for opaque identities that cannot occupy URL path segments.

#[cfg(test)]
mod tests;

use super::{
    get_by_slug_route, get_conversation, get_conversation_message_range, get_conversation_messages,
    get_conversation_messages_around, get_conversation_messages_latest, get_conversation_route,
    get_product_conversation, resolve_coordinator_route, stream_conversation, AppError,
    AroundMessagesQuery, CoordinatorRouteResponse, GetConversationQuery, LatestMessagesQuery,
    MessageHistoryQuery, MessageRangeQuery, StreamConversationQuery,
};
use crate::api::product_conversations::{
    get_coordinator_history, CoordinatorHistoryQuery, OrdinarySnapshotResponse, SnapshotQuery,
};
use crate::api::types::{
    ConversationMessageRangeResponse, ConversationMessageSliceResponse,
    ConversationMessagesAroundResponse, ConversationRouteResponse,
    ConversationWithMessagesResponse,
};
use crate::api::AppState;
use axum::{
    extract::{Path, Query, RawQuery, State},
    response::IntoResponse,
    routing::{get, post},
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
        .route("/api/conversation-reads/route", post(route))
        .route("/api/conversation-reads/route/by-slug", post(route_by_slug))
        .route("/api/conversation-reads/stream", get(stream))
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

async fn route(
    state: State<AppState>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<ConversationRouteResponse>, AppError> {
    get_conversation_route(state, Path(request.reference)).await
}

async fn route_by_slug(
    state: State<AppState>,
    Json(request): Json<ExactReferenceRequest>,
) -> Result<Json<ConversationRouteResponse>, AppError> {
    get_by_slug_route(state, Path(request.reference)).await
}

#[derive(Deserialize)]
struct StreamReferenceQuery {
    reference: String,
}

async fn stream(
    state: State<AppState>,
    RawQuery(raw): RawQuery,
    Query(reference): Query<StreamReferenceQuery>,
    query: Query<StreamConversationQuery>,
) -> Result<impl IntoResponse, AppError> {
    // Axum's form decoder replaces invalid UTF-8; identity must not be repaired.
    validate_query_encoding(raw.as_deref().unwrap_or_default())?;
    stream_conversation(state, Path(reference.reference), query).await
}

fn validate_query_encoding(raw: &str) -> Result<(), AppError> {
    let invalid = || AppError::BadRequest("Invalid stream query encoding".into());
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            let hex = bytes.get(offset + 1..offset + 3).ok_or_else(invalid)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return Err(invalid());
            }
            let hex = std::str::from_utf8(hex).map_err(|_| invalid())?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| invalid())?);
            offset += 3;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
    }
    std::str::from_utf8(&decoded).map_err(|_| invalid())?;
    Ok(())
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
