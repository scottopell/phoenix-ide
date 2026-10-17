use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use futures::StreamExt as _;
use phoenix_core::domain::{
    conversation_reference::ConversationReferenceTarget,
    instance_identity::{FederationCredentialVerifier, InstanceId, PeerTlsTrust},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::future::Future;

use super::auth::{OwnerAuthenticated, PeerAuthenticated};
use super::AppState;

static QUERY_ADMISSION: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));
const MAX_REMOTE_RESPONSE_BYTES: usize = 65 * 1024;

#[derive(Deserialize)]
pub struct IssueEnrollmentRequest {
    pub caller_instance_id: InstanceId,
    pub caller_display_name: String,
}

#[derive(Deserialize, Serialize)]
pub struct EnrollmentTransferBundle {
    pub receiver_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub token: String,
    pub tls_trust: PeerTlsTrust,
}

#[derive(Serialize)]
pub struct RevokeEnrollmentResponse {
    pub revoked: bool,
}

#[derive(Deserialize)]
pub struct ImportPeerConnectionRequest {
    pub peer_display_name: String,
    pub base_url: String,
    pub enrollment: EnrollmentTransferBundle,
}

#[derive(Serialize)]
pub struct ImportPeerConnectionResponse {
    pub peer_instance_id: InstanceId,
}

#[derive(Deserialize, Serialize)]
pub struct RemoteQueryDatabaseRequest {
    pub destination_instance_id: InstanceId,
    pub sql: String,
}

#[derive(Serialize, Deserialize)]
pub struct RemoteQueryDatabaseResponse {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub result: phoenix_db::CoordinatorQueryResult,
}

#[derive(Deserialize, Serialize)]
pub struct RemoteSearchRequest {
    pub caller_instance_id: InstanceId,
    pub destination_instance_id: InstanceId,
    pub query: String,
}

#[derive(Serialize, Deserialize)]
pub struct RemoteSearchResponse {
    pub caller_instance_id: InstanceId,
    pub destination_instance_id: InstanceId,
    pub hits: Vec<crate::api::global_read::QualifiedSearchHit>,
}

#[derive(Serialize, Deserialize)]
pub struct RemoteReadConversationRequest {
    pub caller_instance_id: InstanceId,
    pub destination_instance_id: InstanceId,
    pub target: ConversationReferenceTarget,
    pub cursor: usize,
}

#[derive(Serialize, Deserialize)]
pub struct RemoteReadConversationResponse {
    pub caller_instance_id: InstanceId,
    pub destination_instance_id: InstanceId,
    pub read: crate::api::global_read::QualifiedConversationRead,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RemoteReadErrorCode {
    InvalidTarget,
    DestinationUnavailable,
    ResponseTooLarge,
    CallerMismatch,
    DestinationMismatch,
}

#[derive(Serialize, Deserialize)]
struct RemoteReadErrorResponse {
    code: RemoteReadErrorCode,
    message: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RemoteSearchErrorCode {
    SearchIndexUnavailable,
}

#[derive(Serialize, Deserialize)]
struct RemoteSearchErrorResponse {
    code: RemoteSearchErrorCode,
    message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum FederationClientError {
    #[error("peer connection not found")]
    PeerNotFound,
    #[error("peer database lookup failed: {0}")]
    Database(#[from] phoenix_db::DbError),
    #[error("federation request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("remote response exceeded {0} bytes")]
    ResponseTooLarge(usize),
    #[error("remote query rejected: {0}")]
    RemoteRejected(String),
    #[error("invalid federation response: {0}")]
    InvalidResponse(#[from] serde_json::Error),
    #[error("destination identity mismatch")]
    DestinationMismatch,
    #[error("caller identity mismatch")]
    CallerMismatch,
    #[error("remote search result provenance mismatch")]
    SearchProvenanceMismatch,
    #[error("remote search navigation locator is invalid: {0}")]
    SearchNavigation(#[from] phoenix_core::domain::instance_identity::PeerNavigationUrlError),
    #[error("remote search unavailable: {0}")]
    SearchUnavailable(String),
    #[error("remote read response provenance mismatch")]
    ReadProvenanceMismatch,
    #[error("remote read target rejected: {0}")]
    ReadInvalidTarget(String),
    #[error("remote read destination unavailable: {0}")]
    ReadUnavailable(String),
    #[error("remote read response exceeds the bounded response budget: {0}")]
    ReadResponseTooLarge(String),
}

fn validate_remote_read_provenance(
    response: &RemoteReadConversationResponse,
    target: &ConversationReferenceTarget,
    peer_instance_id: InstanceId,
    caller_instance_id: InstanceId,
) -> Result<(), FederationClientError> {
    if response.destination_instance_id != peer_instance_id {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != caller_instance_id {
        return Err(FederationClientError::CallerMismatch);
    }
    let resolved_matches_request = match (target, &response.read.resolved.target) {
        (
            ConversationReferenceTarget::ProductConversation { .. },
            ConversationReferenceTarget::Transcript { .. },
        ) => true,
        (
            ConversationReferenceTarget::Transcript {
                transcript_id: requested,
            },
            ConversationReferenceTarget::Transcript {
                transcript_id: resolved,
            },
        ) => requested == resolved,
        (
            ConversationReferenceTarget::Message {
                transcript_id: requested_transcript,
                message_id: requested_message,
            },
            ConversationReferenceTarget::Message {
                transcript_id: resolved_transcript,
                message_id: resolved_message,
            },
        ) => requested_transcript == resolved_transcript && requested_message == resolved_message,
        (
            ConversationReferenceTarget::ProductConversation { .. },
            ConversationReferenceTarget::ProductConversation { .. }
            | ConversationReferenceTarget::Message { .. }
            | ConversationReferenceTarget::Coordinator,
        )
        | (
            ConversationReferenceTarget::Transcript { .. },
            ConversationReferenceTarget::ProductConversation { .. }
            | ConversationReferenceTarget::Message { .. }
            | ConversationReferenceTarget::Coordinator,
        )
        | (
            ConversationReferenceTarget::Message { .. },
            ConversationReferenceTarget::ProductConversation { .. }
            | ConversationReferenceTarget::Transcript { .. }
            | ConversationReferenceTarget::Coordinator,
        )
        | (ConversationReferenceTarget::Coordinator, _) => false,
    };
    if response.read.requested.instance_id != peer_instance_id
        || response.read.requested.target != *target
        || response.read.resolved.instance_id != peer_instance_id
        || !resolved_matches_request
    {
        return Err(FederationClientError::ReadProvenanceMismatch);
    }
    Ok(())
}

pub async fn read_remote_conversation(
    local_db: &crate::db::Database,
    peer: &phoenix_db::FederationPeerConnection,
    target: ConversationReferenceTarget,
    cursor: usize,
) -> Result<crate::api::global_read::QualifiedConversationRead, FederationClientError> {
    let caller_instance_id = local_db.instance_id().await?;
    let client = remote_query_client(&peer.tls_trust)?;
    let response = client
        .post(peer.base_url.read_conversation_endpoint().as_url().clone())
        .bearer_auth(peer.bearer_credential.expose())
        .json(&RemoteReadConversationRequest {
            caller_instance_id,
            destination_instance_id: peer.peer_instance_id,
            target: target.clone(),
            cursor,
        })
        .send()
        .await?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, MAX_REMOTE_RESPONSE_BYTES)?;
    }
    let mut response = decode_remote_read_response(status, &bytes)?;
    validate_remote_read_provenance(
        &response,
        &target,
        peer.peer_instance_id,
        caller_instance_id,
    )?;
    let (ConversationReferenceTarget::Transcript {
        transcript_id: resolved_transcript,
    }
    | ConversationReferenceTarget::Message {
        transcript_id: resolved_transcript,
        ..
    }) = &response.read.resolved.target
    else {
        return Err(FederationClientError::ReadProvenanceMismatch);
    };
    if response.read.evidence.iter().any(|reference| {
        reference.instance_id != peer.peer_instance_id
            || !matches!(
                &reference.target,
                ConversationReferenceTarget::Message { transcript_id, .. }
                    if transcript_id == resolved_transcript
            )
    }) {
        return Err(FederationClientError::ReadProvenanceMismatch);
    }
    if let ConversationReferenceTarget::Message {
        message_id: requested_message,
        ..
    } = &target
    {
        if response.read.evidence.len() != 1
            || !matches!(
                &response.read.evidence[0].target,
                ConversationReferenceTarget::Message { message_id, .. }
                    if message_id == requested_message
            )
        {
            return Err(FederationClientError::ReadProvenanceMismatch);
        }
    }
    match (
        &response.read.stable_conversation,
        &response.read.stable_navigation,
    ) {
        (
            Some(stable),
            Some(crate::api::global_read::QualifiedSearchNavigation::AppLocal { .. }),
        ) if stable.instance_id == peer.peer_instance_id
            && (matches!(
                (&target, &stable.target),
                (
                    ConversationReferenceTarget::ProductConversation {
                        product_conversation_id: requested,
                    },
                    ConversationReferenceTarget::ProductConversation {
                        product_conversation_id: returned,
                    },
                ) if requested == returned
            ) || (!matches!(
                target,
                ConversationReferenceTarget::ProductConversation { .. }
            ) && matches!(
                &stable.target,
                ConversationReferenceTarget::ProductConversation { .. }
            ))) => {}
        (None, None) => {}
        _ => return Err(FederationClientError::ReadProvenanceMismatch),
    }
    qualify_remote_read_navigation(&peer.base_url, &mut response.read)?;
    Ok(response.read)
}

fn decode_remote_read_response(
    status: StatusCode,
    bytes: &[u8],
) -> Result<RemoteReadConversationResponse, FederationClientError> {
    if status.is_success() {
        return Ok(serde_json::from_slice(bytes)?);
    }
    let Some(error) = serde_json::from_slice::<RemoteReadErrorResponse>(bytes).ok() else {
        return Err(FederationClientError::RemoteRejected(format!(
            "HTTP {status}"
        )));
    };
    Err(match error.code {
        RemoteReadErrorCode::InvalidTarget => {
            FederationClientError::ReadInvalidTarget(error.message)
        }
        RemoteReadErrorCode::DestinationUnavailable => {
            FederationClientError::ReadUnavailable(error.message)
        }
        RemoteReadErrorCode::ResponseTooLarge => {
            FederationClientError::ReadResponseTooLarge(error.message)
        }
        RemoteReadErrorCode::CallerMismatch => FederationClientError::CallerMismatch,
        RemoteReadErrorCode::DestinationMismatch => FederationClientError::DestinationMismatch,
    })
}

pub async fn search_remote_conversations(
    db: &crate::db::Database,
    peer: &phoenix_db::FederationPeerConnection,
    query: &str,
) -> Result<Vec<crate::api::global_read::QualifiedSearchHit>, FederationClientError> {
    let caller_instance_id = db.instance_id().await?;
    let client = remote_query_client(&peer.tls_trust)?;
    let response = client
        .post(
            peer.base_url
                .search_conversations_endpoint()
                .as_url()
                .clone(),
        )
        .bearer_auth(peer.bearer_credential.expose())
        .json(&RemoteSearchRequest {
            caller_instance_id,
            destination_instance_id: peer.peer_instance_id,
            query: query.to_owned(),
        })
        .send()
        .await?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, MAX_REMOTE_RESPONSE_BYTES)?;
    }
    let mut hits =
        decode_remote_search_response(status, &bytes, peer.peer_instance_id, caller_instance_id)?;
    qualify_remote_search_navigation(&peer.base_url, &mut hits)?;
    Ok(hits)
}

pub async fn query_remote_database(
    db: &crate::db::Database,
    peer_instance_id: InstanceId,
    sql: &str,
) -> Result<RemoteQueryDatabaseResponse, FederationClientError> {
    let peer = db.federation_peer_connection(peer_instance_id).await?;
    let peer = peer.ok_or(FederationClientError::PeerNotFound)?;
    let caller_instance_id = db.instance_id().await?;
    let endpoint = peer.base_url.query_database_endpoint();
    let client = remote_query_client(&peer.tls_trust)?;
    let response = client
        .post(endpoint.as_url().clone())
        .bearer_auth(peer.bearer_credential.expose())
        .json(&RemoteQueryDatabaseRequest {
            destination_instance_id: peer_instance_id,
            sql: sql.to_owned(),
        })
        .send()
        .await?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, MAX_REMOTE_RESPONSE_BYTES)?;
    }
    decode_remote_query_response(status, &bytes, peer_instance_id, caller_instance_id)
}

fn remote_query_client(tls_trust: &PeerTlsTrust) -> Result<reqwest::Client, FederationClientError> {
    let builder = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30));
    let builder = match tls_trust {
        PeerTlsTrust::PlatformRoots => builder,
        PeerTlsTrust::PrivateCa { certificate_pem } => {
            builder.tls_certs_only([reqwest::Certificate::from_pem(
                certificate_pem.expose().as_bytes(),
            )?])
        }
    };
    Ok(builder.build()?)
}

fn append_bounded_response_chunk(
    body: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
) -> Result<(), FederationClientError> {
    if body.len().saturating_add(chunk.len()) > max_bytes {
        return Err(FederationClientError::ResponseTooLarge(max_bytes));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn decode_remote_query_response(
    status: StatusCode,
    bytes: &[u8],
    expected_destination: InstanceId,
    expected_caller: InstanceId,
) -> Result<RemoteQueryDatabaseResponse, FederationClientError> {
    if !status.is_success() {
        let detail = serde_json::from_slice::<serde_json::Value>(bytes)
            .ok()
            .and_then(|body| {
                body.get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(FederationClientError::RemoteRejected(detail));
    }
    let response: RemoteQueryDatabaseResponse = serde_json::from_slice(bytes)?;
    if response.destination_instance_id != expected_destination {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != expected_caller {
        return Err(FederationClientError::CallerMismatch);
    }
    Ok(response)
}

fn qualify_remote_read_navigation(
    base_url: &phoenix_core::domain::instance_identity::PeerBaseUrl,
    read: &mut crate::api::global_read::QualifiedConversationRead,
) -> Result<(), FederationClientError> {
    use crate::api::global_read::QualifiedSearchNavigation;

    let QualifiedSearchNavigation::AppLocal { path } = &read.source_navigation else {
        return Err(FederationClientError::ReadProvenanceMismatch);
    };
    let url = base_url.navigation_url(path)?.as_url().to_string();
    read.source_navigation = QualifiedSearchNavigation::Peer { url };
    if let Some(navigation) = &read.stable_navigation {
        let QualifiedSearchNavigation::AppLocal { path } = navigation else {
            return Err(FederationClientError::ReadProvenanceMismatch);
        };
        let url = base_url.navigation_url(path)?.as_url().to_string();
        read.stable_navigation = Some(QualifiedSearchNavigation::Peer { url });
    }
    Ok(())
}

fn qualify_remote_search_navigation(
    base_url: &phoenix_core::domain::instance_identity::PeerBaseUrl,
    hits: &mut [crate::api::global_read::QualifiedSearchHit],
) -> Result<(), FederationClientError> {
    use crate::api::global_read::QualifiedSearchNavigation;

    for hit in hits {
        let Some(QualifiedSearchNavigation::AppLocal { path }) = &hit.source_navigation else {
            return Err(FederationClientError::SearchProvenanceMismatch);
        };
        let url = base_url.navigation_url(path)?.as_url().to_string();
        hit.source_navigation = Some(QualifiedSearchNavigation::Peer { url });

        match (&hit.stable_conversation, &hit.stable_navigation) {
            (None, None) => {}
            (Some(_), Some(QualifiedSearchNavigation::AppLocal { path })) => {
                let url = base_url.navigation_url(path)?.as_url().to_string();
                hit.stable_navigation = Some(QualifiedSearchNavigation::Peer { url });
            }
            _ => return Err(FederationClientError::SearchProvenanceMismatch),
        }
    }
    Ok(())
}

fn decode_remote_search_response(
    status: StatusCode,
    bytes: &[u8],
    expected_destination: InstanceId,
    expected_caller: InstanceId,
) -> Result<Vec<crate::api::global_read::QualifiedSearchHit>, FederationClientError> {
    if !status.is_success() {
        let error = serde_json::from_slice::<RemoteSearchErrorResponse>(bytes).ok();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            if let Some(RemoteSearchErrorResponse {
                code: RemoteSearchErrorCode::SearchIndexUnavailable,
                message,
            }) = &error
            {
                return Err(FederationClientError::SearchUnavailable(message.clone()));
            }
        }
        let detail = error
            .map(|error| error.message)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(FederationClientError::RemoteRejected(detail));
    }
    let response: RemoteSearchResponse = serde_json::from_slice(bytes)?;
    if response.destination_instance_id != expected_destination {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != expected_caller {
        return Err(FederationClientError::CallerMismatch);
    }
    if response.hits.iter().any(|hit| {
        hit.source.instance_id != expected_destination
            || !matches!(
                &hit.source_navigation,
                Some(crate::api::global_read::QualifiedSearchNavigation::AppLocal { .. })
            )
            || !matches!(
                &hit.source.target,
                ConversationReferenceTarget::Message { .. }
            )
            || match (&hit.stable_conversation, &hit.stable_navigation) {
                (None, None) => false,
                (
                    Some(stable),
                    Some(crate::api::global_read::QualifiedSearchNavigation::AppLocal { .. }),
                ) => {
                    stable.instance_id != expected_destination
                        || !matches!(
                            &stable.target,
                            ConversationReferenceTarget::ProductConversation { .. }
                        )
                }
                _ => true,
            }
            || hit.sender.as_ref().is_some_and(|sender| {
                sender.stable.instance_id != expected_destination
                    || !matches!(
                        &sender.stable.target,
                        ConversationReferenceTarget::ProductConversation { .. }
                            | ConversationReferenceTarget::Coordinator
                    )
                    || sender.exact.instance_id != expected_destination
                    || !matches!(
                        &sender.exact.target,
                        ConversationReferenceTarget::Transcript { .. }
                    )
            })
    }) {
        return Err(FederationClientError::SearchProvenanceMismatch);
    }
    Ok(response.hits)
}

fn random_peer_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "phx_peer_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

pub async fn read_conversation(
    authenticated: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteReadConversationRequest>,
) -> Response {
    let destination = match state.db.instance_id().await {
        Ok(instance_id) => instance_id,
        Err(error) => {
            tracing::error!(%error, "failed to read local instance identity");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if request.destination_instance_id != destination {
        return (
            StatusCode::CONFLICT,
            Json(RemoteReadErrorResponse {
                code: RemoteReadErrorCode::DestinationMismatch,
                message: "destination instance identity mismatch".to_string(),
            }),
        )
            .into_response();
    }
    if request.caller_instance_id != authenticated.caller_instance_id {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(RemoteReadErrorResponse {
                code: RemoteReadErrorCode::CallerMismatch,
                message: "caller instance identity mismatch".to_string(),
            }),
        )
            .into_response();
    }
    if matches!(request.target, ConversationReferenceTarget::Coordinator) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(RemoteReadErrorResponse {
                code: RemoteReadErrorCode::InvalidTarget,
                message: "Coordinator is not a readable conversation target".to_string(),
            }),
        )
            .into_response();
    }
    let service = crate::api::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    match service
        .read_conversation_target(&request.target, request.cursor)
        .await
    {
        Ok(read) => {
            let response = RemoteReadConversationResponse {
                caller_instance_id: authenticated.caller_instance_id,
                destination_instance_id: destination,
                read,
            };
            match serde_json::to_vec(&response) {
                Ok(encoded) if encoded.len() <= MAX_REMOTE_RESPONSE_BYTES => {
                    Json(response).into_response()
                }
                Ok(_) => (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Json(RemoteReadErrorResponse {
                        code: RemoteReadErrorCode::ResponseTooLarge,
                        message: "remote read response exceeds the bounded response budget"
                            .to_string(),
                    }),
                )
                    .into_response(),
                Err(error) => {
                    tracing::error!(%error, "failed to encode peer conversation read");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(crate::api::global_read::QualifiedReadError::Invalid(error)) => {
            tracing::debug!(%error, "peer conversation read rejected");
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(RemoteReadErrorResponse {
                    code: RemoteReadErrorCode::InvalidTarget,
                    message: error,
                }),
            )
                .into_response()
        }
        Err(crate::api::global_read::QualifiedReadError::Failed(error)) => {
            tracing::error!(%error, "peer conversation read failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RemoteReadErrorResponse {
                    code: RemoteReadErrorCode::DestinationUnavailable,
                    message: "destination conversation read unavailable".to_string(),
                }),
            )
                .into_response()
        }
    }
}

pub async fn search_conversations(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteSearchRequest>,
) -> Response {
    let local_id = match state.db.instance_id().await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "failed to read local instance identity for peer search");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if request.destination_instance_id != local_id {
        return StatusCode::CONFLICT.into_response();
    }
    if request.caller_instance_id != peer.caller_instance_id || request.query.trim().is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    let read = crate::api::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    match read.qualified_search(request.query.trim()).await {
        Ok(hits) => Json(RemoteSearchResponse {
            caller_instance_id: peer.caller_instance_id,
            destination_instance_id: local_id,
            hits,
        })
        .into_response(),
        Err(crate::api::global_read::QualifiedSearchError::Unavailable(message)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RemoteSearchErrorResponse {
                code: RemoteSearchErrorCode::SearchIndexUnavailable,
                message: message.to_string(),
            }),
        )
            .into_response(),
        Err(crate::api::global_read::QualifiedSearchError::Failed(error)) => {
            tracing::error!(%error, "peer search failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn query_database(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteQueryDatabaseRequest>,
) -> Response {
    query_database_with_admission(peer, state, request, &QUERY_ADMISSION).await
}

async fn query_database_with_admission(
    peer: PeerAuthenticated,
    state: AppState,
    request: RemoteQueryDatabaseRequest,
    admission: &std::sync::Arc<tokio::sync::Semaphore>,
) -> Response {
    let destination_instance_id = match state.db.instance_id().await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "failed to read destination instance identity");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if request.destination_instance_id != destination_instance_id {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "destination instance mismatch" })),
        )
            .into_response();
    }
    let Ok(permit) = std::sync::Arc::clone(admission).try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "remote query admission limit reached" })),
        )
            .into_response();
    };
    let service = super::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    let sql = request.sql;
    match spawn_with_admission_permit(permit, async move { service.query_database(&sql).await })
        .await
    {
        Ok(Ok(result)) => Json(RemoteQueryDatabaseResponse {
            destination_instance_id,
            caller_instance_id: peer.caller_instance_id,
            result,
        })
        .into_response(),
        Ok(Err(error)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": error,
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "remote query task failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn spawn_with_admission_permit<T, F>(
    permit: tokio::sync::OwnedSemaphorePermit,
    work: F,
) -> tokio::task::JoinHandle<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    tokio::spawn(async move {
        let _permit = permit;
        work.await
    })
}

pub async fn issue_enrollment(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<IssueEnrollmentRequest>,
) -> Response {
    let local_id = match state.db.instance_id().await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "failed to read local instance identity");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if request.caller_instance_id == local_id || request.caller_display_name.trim().is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    let token = random_peer_token();
    let verifier = FederationCredentialVerifier::from_bearer(token.as_bytes());
    if let Err(error) = state
        .db
        .replace_federation_enrollment(
            request.caller_instance_id,
            request.caller_display_name.trim(),
            &verifier,
        )
        .await
    {
        tracing::error!(%error, "failed to issue federation enrollment");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    (
        StatusCode::CREATED,
        Json(EnrollmentTransferBundle {
            receiver_instance_id: local_id,
            caller_instance_id: request.caller_instance_id,
            token,
            tls_trust: state.federation_tls_trust.clone(),
        }),
    )
        .into_response()
}

pub async fn import_peer_connection(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<ImportPeerConnectionRequest>,
) -> Response {
    let local_id = match state.db.instance_id().await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "failed to read local instance identity");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if request.enrollment.caller_instance_id != local_id
        || request.enrollment.receiver_instance_id == local_id
        || request.peer_display_name.trim().is_empty()
    {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    let Ok(base_url) = request.base_url.parse() else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    let Ok(bearer_credential) =
        phoenix_core::domain::instance_identity::PeerBearerCredential::parse(
            request.enrollment.token,
        )
    else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    match state
        .db
        .save_federation_peer_connection(
            request.enrollment.receiver_instance_id,
            request.peer_display_name.trim(),
            &base_url,
            &bearer_credential,
            &request.enrollment.tls_trust,
        )
        .await
    {
        Ok(()) => Json(ImportPeerConnectionResponse {
            peer_instance_id: request.enrollment.receiver_instance_id,
        })
        .into_response(),
        Err(error) => {
            tracing::error!(%error, "failed to import federation peer connection");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn revoke_enrollment(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Path(caller_instance_id): Path<InstanceId>,
) -> Response {
    match state
        .db
        .revoke_federation_enrollment(caller_instance_id)
        .await
    {
        Ok(revoked) => Json(RevokeEnrollmentResponse { revoked }).into_response(),
        Err(error) => {
            tracing::error!(%error, "failed to revoke federation enrollment");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_tokens_are_prefixed_random_credentials() {
        let first = random_peer_token();
        let second = random_peer_token();
        assert!(first.starts_with("phx_peer_"));
        assert_ne!(first, second);
        assert_eq!(first.len(), "phx_peer_".len() + 43);
    }

    #[tokio::test]
    async fn peer_import_rejects_bundle_for_another_caller() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let response = import_peer_connection(
            OwnerAuthenticated,
            State(state),
            Json(ImportPeerConnectionRequest {
                peer_display_name: "peer".to_string(),
                base_url: "https://peer.example".to_string(),
                enrollment: EnrollmentTransferBundle {
                    receiver_instance_id: InstanceId::new(),
                    caller_instance_id: InstanceId::new(),
                    token: format!("phx_peer_{}", "a".repeat(43)),
                    tls_trust: PeerTlsTrust::PlatformRoots,
                },
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn private_ca_trust_is_isolated_to_the_selected_peer() {
        let trusted = tempfile::tempdir().unwrap();
        let trusted = crate::tls::load_config(&crate::tls::ConfigSource::Auto {
            dir: trusted.path().to_path_buf(),
            hosts: vec!["localhost".to_string()],
        })
        .unwrap();
        let certificate_pem = phoenix_core::domain::instance_identity::PeerCaCertificatePem::parse(
            std::fs::read_to_string(trusted.ca_cert_path.unwrap()).unwrap(),
        )
        .unwrap();
        let client = remote_query_client(&PeerTlsTrust::PrivateCa { certificate_pem }).unwrap();

        let other = tempfile::tempdir().unwrap();
        let other = crate::tls::load_config(&crate::tls::ConfigSource::Auto {
            dir: other.path().to_path_buf(),
            hosts: vec!["localhost".to_string()],
        })
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(other.server));
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            assert!(acceptor.accept(stream).await.is_err());
        });

        assert!(client
            .get(format!("https://localhost:{}/", address.port()))
            .send()
            .await
            .is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn authenticated_remote_query_does_not_follow_307_or_308() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let peer = InstanceId::new();
        let token = phoenix_core::domain::instance_identity::PeerBearerCredential::parse(format!(
            "phx_peer_{}",
            "a".repeat(43)
        ))
        .unwrap();

        for (status, cross_origin) in [
            ("307 Temporary Redirect", false),
            ("308 Permanent Redirect", true),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let mut loaded = crate::tls::load_config(&crate::tls::ConfigSource::Auto {
                dir: temp.path().to_path_buf(),
                hosts: vec!["localhost".to_string()],
            })
            .unwrap();
            loaded.server.alpn_protocols = vec![b"http/1.1".to_vec()];
            let certificate_pem =
                phoenix_core::domain::instance_identity::PeerCaCertificatePem::parse(
                    std::fs::read_to_string(loaded.ca_cert_path.unwrap()).unwrap(),
                )
                .unwrap();
            let trust = PeerTlsTrust::PrivateCa { certificate_pem };
            let source = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let source_address = source.local_addr().unwrap();
            let target = if cross_origin {
                Some(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap())
            } else {
                None
            };
            let redirect_port = target.as_ref().map_or(source_address.port(), |listener| {
                listener.local_addr().unwrap().port()
            });
            let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(loaded.server));
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
                let (stream, _) = source.accept().await.unwrap();
                let stream = acceptor.accept(stream).await.unwrap();
                let mut stream = tokio::io::BufReader::new(stream);
                let mut request = Vec::new();
                stream.read_until(b'\n', &mut request).await.unwrap();
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_until(b'\n', &mut request).await.unwrap();
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("POST /api/federation/peer/query-database "));
                assert!(request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer "));
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nLocation: https://localhost:{redirect_port}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                drop(stream);

                tokio::select! {
                    accepted = async {
                        match target {
                            Some(target) => target.accept().await,
                            None => source.accept().await,
                        }
                    } => {
                        accepted.unwrap();
                        true
                    }
                    result = done_rx => {
                        result.unwrap();
                        false
                    }
                }
            });
            state
                .db
                .save_federation_peer_connection(
                    peer,
                    "peer",
                    &format!("https://localhost:{}", source_address.port())
                        .parse()
                        .unwrap(),
                    &token,
                    &trust,
                )
                .await
                .unwrap();

            let result = query_remote_database(&state.db, peer, "SELECT 1").await;
            assert!(matches!(
                result,
                Err(FederationClientError::RemoteRejected(detail))
                    if detail.starts_with("HTTP 30")
            ));
            done_tx.send(()).unwrap();
            assert!(!server.await.unwrap(), "redirect target received a request");
        }
    }

    #[test]
    fn remote_response_rejects_wrong_caller_identity() {
        let destination = InstanceId::new();
        let expected_caller = InstanceId::new();
        let body = serde_json::to_vec(&RemoteQueryDatabaseResponse {
            destination_instance_id: destination,
            caller_instance_id: InstanceId::new(),
            result: phoenix_db::CoordinatorQueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                truncated: false,
                row_limit: 200,
                byte_limit: 64 * 1024,
                elapsed_ms: 0,
            },
        })
        .unwrap();

        assert!(matches!(
            decode_remote_query_response(StatusCode::OK, &body, destination, expected_caller),
            Err(FederationClientError::CallerMismatch)
        ));
    }

    #[test]
    fn remote_search_rejects_hit_from_wrong_source_instance() {
        use phoenix_core::domain::{
            close::TranscriptConversationId,
            conversation_reference::{MessageId, QualifiedConversationReference},
        };

        let destination = InstanceId::new();
        let caller = InstanceId::new();
        let body = serde_json::to_vec(&RemoteSearchResponse {
            destination_instance_id: destination,
            caller_instance_id: caller,
            hits: vec![crate::api::global_read::QualifiedSearchHit {
                source: QualifiedConversationReference::new(
                    InstanceId::new(),
                    ConversationReferenceTarget::Message {
                        transcript_id: TranscriptConversationId::parse("remote-transcript")
                            .unwrap(),
                        message_id: MessageId::parse("remote-message").unwrap(),
                    },
                ),
                source_navigation: Some(
                    crate::api::global_read::QualifiedSearchNavigation::AppLocal {
                        path:
                            "/c/remote?source_transcript=remote-transcript#message-remote-message"
                                .to_string(),
                    },
                ),
                stable_conversation: None,
                stable_navigation: None,
                role: "assistant".to_string(),
                sender: None,
                created_at: chrono::Utc::now(),
                snippet: "evidence".to_string(),
            }],
        })
        .unwrap();

        assert!(matches!(
            decode_remote_search_response(StatusCode::OK, &body, destination, caller),
            Err(FederationClientError::SearchProvenanceMismatch)
        ));
    }

    #[test]
    fn remote_response_limit_applies_across_streamed_chunks() {
        let mut body = Vec::new();
        append_bounded_response_chunk(&mut body, &[0; 4], 8).unwrap();
        append_bounded_response_chunk(&mut body, &[0; 4], 8).unwrap();
        let error = append_bounded_response_chunk(&mut body, &[0], 8).unwrap_err();

        assert!(matches!(error, FederationClientError::ResponseTooLarge(8)));
        assert_eq!(body.len(), 8);
    }

    #[test]
    fn remote_rejection_preserves_server_error_detail() {
        let expected_destination = InstanceId::new();
        let result = decode_remote_query_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            br#"{"error":"specific SQL diagnostic"}"#,
            expected_destination,
            InstanceId::new(),
        );

        assert!(matches!(
            result,
            Err(FederationClientError::RemoteRejected(detail))
                if detail == "specific SQL diagnostic"
        ));
    }

    #[test]
    fn remote_query_request_uses_shared_wire_shape() {
        let destination_instance_id = InstanceId::new();
        let request = RemoteQueryDatabaseRequest {
            destination_instance_id,
            sql: "SELECT 1".to_string(),
        };

        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "destination_instance_id": destination_instance_id,
                "sql": "SELECT 1",
            })
        );
        let decoded: RemoteQueryDatabaseRequest = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.destination_instance_id, destination_instance_id);
        assert_eq!(decoded.sql, "SELECT 1");
    }

    #[tokio::test]
    async fn cancelled_caller_does_not_release_admission_before_work_finishes() {
        let admission = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = std::sync::Arc::clone(&admission)
            .try_acquire_owned()
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let task = spawn_with_admission_permit(permit, async move {
            started_tx.send(()).unwrap();
            finish_rx.await.unwrap();
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), started_rx)
            .await
            .unwrap()
            .unwrap();

        let mut caller = Box::pin(task);
        assert!(futures::poll!(&mut caller).is_pending());
        drop(caller);
        assert!(admission.clone().try_acquire_owned().is_err());

        finish_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), admission.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }

    #[tokio::test]
    async fn query_database_rejects_exhausted_admission_before_sql_execution() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let destination_instance_id = state.db.instance_id().await.unwrap();
        let admission = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let response = query_database_with_admission(
            PeerAuthenticated {
                caller_instance_id: InstanceId::new(),
                caller_display_name: "peer".to_string(),
            },
            state,
            RemoteQueryDatabaseRequest {
                destination_instance_id,
                sql: "not valid SQL".to_string(),
            },
            &admission,
        )
        .await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({ "error": "remote query admission limit reached" })
        );
    }
}
