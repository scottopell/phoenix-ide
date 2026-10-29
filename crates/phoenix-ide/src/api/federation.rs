use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use futures::StreamExt as _;
use phoenix_core::domain::instance_identity::{
    FederationCredentialVerifier, InstanceId, PeerTlsTrust,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::str::FromStr as _;

use super::auth::{OwnerAuthenticated, PeerAuthenticated};
use super::AppState;

static QUERY_ADMISSION: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));
static READ_ADMISSION: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));
const REMOTE_READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
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

#[derive(Debug, thiserror::Error)]
pub enum FederationClientError {
    #[error("peer connection not found")]
    PeerNotFound,
    #[error("peer display name is ambiguous")]
    PeerAmbiguous,
    #[error("peer selector must not be empty")]
    InvalidPeerSelector,
    #[error("peer database lookup failed: {0}")]
    Database(#[from] phoenix_db::DbError),
    #[error("peer selection failed: {0}")]
    Sql(#[from] sqlx::Error),
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
    #[error("reference provenance mismatch: {0}")]
    ProvenanceMismatch(String),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteConversationTarget {
    ProductConversation {
        product_conversation_id: String,
    },
    ExactTranscript {
        transcript_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },
}

impl RemoteConversationTarget {
    fn parse(reference: &str) -> Result<Self, FederationClientError> {
        let reference = reference.trim();
        if reference.len() > 1024 || reference.chars().any(char::is_whitespace) {
            return Err(FederationClientError::ProvenanceMismatch(
                "reference must be one bounded canonical token".to_string(),
            ));
        }
        if let Some(id) = reference.strip_prefix("@conv:") {
            if id.is_empty() || id.contains('#') {
                return Err(FederationClientError::ProvenanceMismatch(
                    "stable reference must be @conv:<product_conversation_id>".to_string(),
                ));
            }
            return Ok(Self::ProductConversation {
                product_conversation_id: id.to_string(),
            });
        }
        if let Some(rest) = reference.strip_prefix("@transcript:") {
            let (transcript_id, message_id) = match rest.split_once('#') {
                Some((id, fragment)) => (
                    id,
                    Some(
                        fragment
                            .strip_prefix("message-")
                            .filter(|value| !value.is_empty() && !value.contains('#'))
                            .ok_or_else(|| {
                                FederationClientError::ProvenanceMismatch(
                                    "transcript fragment must be #message-<message_id>".to_string(),
                                )
                            })?,
                    ),
                ),
                None => (rest, None),
            };
            if transcript_id.is_empty() {
                return Err(FederationClientError::ProvenanceMismatch(
                    "exact reference must name a transcript".to_string(),
                ));
            }
            return Ok(Self::ExactTranscript {
                transcript_id: transcript_id.to_string(),
                message_id: message_id.map(str::to_string),
            });
        }
        Err(FederationClientError::ProvenanceMismatch(
            "remote reference must be @conv: or @transcript:".to_string(),
        ))
    }

    fn reference(&self) -> String {
        match self {
            Self::ProductConversation {
                product_conversation_id,
            } => format!("@conv:{product_conversation_id}"),
            Self::ExactTranscript {
                transcript_id,
                message_id,
            } => message_id.as_ref().map_or_else(
                || format!("@transcript:{transcript_id}"),
                |message_id| format!("@transcript:{transcript_id}#message-{message_id}"),
            ),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct RemoteResolveReferenceRequest {
    pub destination_instance_id: InstanceId,
    pub target: RemoteConversationTarget,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteResolveReferenceResponse {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub path_owner_instance_id: InstanceId,
    pub requested: RemoteConversationTarget,
    pub resolution: super::global_read::ResolveGlobalReferenceResponse,
}

#[derive(Serialize, Deserialize)]
pub struct RemoteReadConversationRequest {
    pub destination_instance_id: InstanceId,
    pub target: RemoteConversationTarget,
    #[serde(default)]
    pub cursor: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteReadConversationResponse {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub source_instance_id: InstanceId,
    pub requested: RemoteConversationTarget,
    pub cursor: usize,
    pub next_cursor: Option<usize>,
    pub content: String,
}

pub async fn resolve_saved_peer_id(
    db: &crate::db::Database,
    selector: &str,
) -> Result<InstanceId, FederationClientError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(FederationClientError::InvalidPeerSelector);
    }
    if let Ok(id) = InstanceId::from_str(selector) {
        return db
            .federation_peer_connection(id)
            .await?
            .map(|_| id)
            .ok_or(FederationClientError::PeerNotFound);
    }
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT peer_instance_id FROM federation_peer_connections
         WHERE peer_display_name = ?1 ORDER BY peer_instance_id LIMIT 2",
    )
    .bind(selector)
    .fetch_all(db.pool())
    .await?;
    let candidates = ids
        .iter()
        .map(|id| InstanceId::from_str(id))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| phoenix_db::DbError::Serialization(error.to_string()))?;
    match candidates.as_slice() {
        [] => Err(FederationClientError::PeerNotFound),
        [id] => Ok(*id),
        _ => Err(FederationClientError::PeerAmbiguous),
    }
}

async fn send_bounded_peer_request<T: serde::de::DeserializeOwned>(
    endpoint: String,
    bearer: &str,
    tls_trust: &PeerTlsTrust,
    body: &impl Serialize,
) -> Result<T, FederationClientError> {
    let response = remote_query_client(tls_trust)?
        .post(endpoint)
        .bearer_auth(bearer)
        .json(body)
        .send()
        .await?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, MAX_REMOTE_RESPONSE_BYTES)?;
    }
    if !status.is_success() {
        let detail = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(FederationClientError::RemoteRejected(detail));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn resolve_remote_reference(
    db: &crate::db::Database,
    selector: &str,
    reference: &str,
) -> Result<RemoteResolveReferenceResponse, FederationClientError> {
    let requested = RemoteConversationTarget::parse(reference)?;
    let peer_id = resolve_saved_peer_id(db, selector).await?;
    let peer = db
        .federation_peer_connection(peer_id)
        .await?
        .ok_or(FederationClientError::PeerNotFound)?;
    let caller = db.instance_id().await?;
    let mut response: RemoteResolveReferenceResponse = send_bounded_peer_request(
        format!("{}api/federation/peer/resolve-reference", peer.base_url),
        peer.bearer_credential.expose(),
        &peer.tls_trust,
        &RemoteResolveReferenceRequest {
            destination_instance_id: peer_id,
            target: requested.clone(),
        },
    )
    .await?;
    validate_remote_provenance(&response, peer_id, caller, &requested)?;
    validate_resolution_binding(&response.resolution, &requested)?;
    response.resolution.href = None;
    Ok(response)
}

fn require_canonical_target(
    target: &RemoteConversationTarget,
) -> Result<(), FederationClientError> {
    if RemoteConversationTarget::parse(&target.reference())? != *target {
        return Err(FederationClientError::ProvenanceMismatch(
            "remote target fields are not canonical".to_string(),
        ));
    }
    Ok(())
}

fn require_bounded_read_target(
    target: &RemoteConversationTarget,
) -> Result<(), FederationClientError> {
    require_canonical_target(target)?;
    if matches!(
        target,
        RemoteConversationTarget::ExactTranscript {
            message_id: Some(_),
            ..
        }
    ) {
        return Err(FederationClientError::RemoteRejected(String::from(
            "remote message reads are not paged; use exact transcript cursors",
        )));
    }
    Ok(())
}

pub async fn read_remote_conversation(
    db: &crate::db::Database,
    selector: &str,
    reference: &str,
    cursor: usize,
) -> Result<RemoteReadConversationResponse, FederationClientError> {
    if cursor > super::global_read::MAX_CONVERSATION_READ_CURSOR {
        return Err(FederationClientError::RemoteRejected(format!(
            "cursor exceeds maximum {}",
            super::global_read::MAX_CONVERSATION_READ_CURSOR
        )));
    }
    let requested = RemoteConversationTarget::parse(reference)?;
    require_bounded_read_target(&requested)?;
    let peer_id = resolve_saved_peer_id(db, selector).await?;
    let peer = db
        .federation_peer_connection(peer_id)
        .await?
        .ok_or(FederationClientError::PeerNotFound)?;
    let caller = db.instance_id().await?;
    let response: RemoteReadConversationResponse = send_bounded_peer_request(
        format!("{}api/federation/peer/read-conversation", peer.base_url),
        peer.bearer_credential.expose(),
        &peer.tls_trust,
        &RemoteReadConversationRequest {
            destination_instance_id: peer_id,
            target: requested.clone(),
            cursor,
        },
    )
    .await?;
    if response.destination_instance_id != peer_id || response.source_instance_id != peer_id {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != caller {
        return Err(FederationClientError::CallerMismatch);
    }
    if response.requested != requested {
        return Err(FederationClientError::ProvenanceMismatch(
            "read response changed the requested reference".to_string(),
        ));
    }
    if response.cursor != cursor {
        return Err(FederationClientError::ProvenanceMismatch(
            "read response changed the requested cursor".to_string(),
        ));
    }
    validate_remote_next_cursor(cursor, response.next_cursor)?;
    Ok(response)
}

fn validate_remote_next_cursor(
    cursor: usize,
    next_cursor: Option<usize>,
) -> Result<(), FederationClientError> {
    if let Some(next_cursor) = next_cursor {
        if next_cursor <= cursor || next_cursor > super::global_read::MAX_CONVERSATION_READ_CURSOR {
            return Err(FederationClientError::ProvenanceMismatch(
                "read response returned an invalid next cursor".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_remote_provenance(
    response: &RemoteResolveReferenceResponse,
    destination: InstanceId,
    caller: InstanceId,
    requested: &RemoteConversationTarget,
) -> Result<(), FederationClientError> {
    if response.destination_instance_id != destination
        || response.path_owner_instance_id != destination
    {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != caller {
        return Err(FederationClientError::CallerMismatch);
    }
    if response.requested != *requested {
        return Err(FederationClientError::ProvenanceMismatch(
            "resolve response changed the requested reference".to_string(),
        ));
    }
    Ok(())
}

fn validate_resolution_binding(
    resolution: &super::global_read::ResolveGlobalReferenceResponse,
    requested: &RemoteConversationTarget,
) -> Result<(), FederationClientError> {
    let (expected_kind, expected_id) = match requested {
        RemoteConversationTarget::ProductConversation {
            product_conversation_id,
        } => ("product_conversation", product_conversation_id.as_str()),
        RemoteConversationTarget::ExactTranscript {
            transcript_id,
            message_id: None,
        } => ("conversation", transcript_id.as_str()),
        RemoteConversationTarget::ExactTranscript {
            message_id: Some(message_id),
            ..
        } => ("message", message_id.as_str()),
    };
    let href = resolution.href.as_deref().ok_or_else(|| {
        FederationClientError::ProvenanceMismatch("resolved reference omitted href".to_string())
    })?;
    if !href.starts_with('/') || href.starts_with("//") {
        return Err(FederationClientError::ProvenanceMismatch(
            "resolved href is not destination-relative".to_string(),
        ));
    }
    let parsed_href = reqwest::Url::parse("https://destination.invalid/")
        .and_then(|base| base.join(href))
        .map_err(|error| FederationClientError::ProvenanceMismatch(error.to_string()))?;
    if parsed_href.scheme() != "https" || parsed_href.host_str() != Some("destination.invalid") {
        return Err(FederationClientError::ProvenanceMismatch(
            "resolved href is not destination-relative".to_string(),
        ));
    }
    if let RemoteConversationTarget::ExactTranscript {
        transcript_id,
        message_id,
    } = requested
    {
        let transcript_href = super::global_read::exact_conversation_href(transcript_id, None);
        let message_href = message_id.as_deref().map(|message_id| {
            super::global_read::exact_conversation_href(transcript_id, Some(message_id))
        });
        if href != transcript_href && message_href.as_deref() != Some(href) {
            return Err(FederationClientError::ProvenanceMismatch(
                "resolved href changed the exact transcript target".to_string(),
            ));
        }
    }
    if resolution.kind.as_str() != expected_kind || resolution.id != expected_id {
        return Err(FederationClientError::ProvenanceMismatch(
            "resolved kind or identity does not match the requested reference".to_string(),
        ));
    }
    Ok(())
}

pub async fn resolve_reference(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteResolveReferenceRequest>,
) -> Response {
    let destination = match state.db.instance_id().await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "failed to read destination instance identity");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if request.destination_instance_id != destination {
        return federation_error(StatusCode::CONFLICT, "destination instance mismatch");
    }
    if let Err(error) = require_canonical_target(&request.target) {
        return federation_error(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string());
    }
    let service = super::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    let reference = request.target.reference();
    let resolution = match service.resolve_reference(&reference).await {
        Ok(resolution) => resolution,
        Err(error) => return error.into_response(),
    };
    Json(RemoteResolveReferenceResponse {
        destination_instance_id: destination,
        caller_instance_id: peer.caller_instance_id,
        path_owner_instance_id: destination,
        requested: request.target,
        resolution,
    })
    .into_response()
}

pub async fn read_conversation(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteReadConversationRequest>,
) -> Response {
    read_conversation_with_limits(
        peer,
        state,
        request,
        &READ_ADMISSION,
        REMOTE_READ_DEADLINE,
    )
    .await
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteReadLimitError {
    Admission,
    Deadline,
}

async fn execute_bounded_remote_read<T, F>(
    admission: &std::sync::Arc<tokio::sync::Semaphore>,
    deadline: std::time::Duration,
    operation: F,
) -> Result<T, RemoteReadLimitError>
where
    F: Future<Output = T>,
{
    let _permit = std::sync::Arc::clone(admission)
        .try_acquire_owned()
        .map_err(|_| RemoteReadLimitError::Admission)?;
    tokio::time::timeout(deadline, operation)
        .await
        .map_err(|_| RemoteReadLimitError::Deadline)
}

async fn read_conversation_with_limits(
    peer: PeerAuthenticated,
    state: AppState,
    request: RemoteReadConversationRequest,
    admission: &std::sync::Arc<tokio::sync::Semaphore>,
    deadline: std::time::Duration,
) -> Response {
    let destination = match state.db.instance_id().await {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "failed to read destination instance identity");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if request.destination_instance_id != destination {
        return federation_error(StatusCode::CONFLICT, "destination instance mismatch");
    }
    if let Err(error) = require_bounded_read_target(&request.target) {
        return federation_error(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string());
    }
    let service = super::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    let read = execute_bounded_remote_read(
        admission,
        deadline,
        service.read_conversation_page(&request.target.reference(), request.cursor),
    )
    .await;
    match read {
        Err(RemoteReadLimitError::Admission) => federation_error(
            StatusCode::TOO_MANY_REQUESTS,
            "remote read admission limit reached",
        ),
        Err(RemoteReadLimitError::Deadline) => {
            federation_error(StatusCode::GATEWAY_TIMEOUT, "remote read deadline exceeded")
        }
        Ok(Ok(page)) => Json(RemoteReadConversationResponse {
            destination_instance_id: destination,
            caller_instance_id: peer.caller_instance_id,
            source_instance_id: destination,
            requested: request.target,
            cursor: request.cursor,
            next_cursor: page.next_cursor,
            content: page.content,
        })
        .into_response(),
        Ok(Err(error)) => federation_error(StatusCode::UNPROCESSABLE_ENTITY, &error),
    }
}

fn federation_error(status: StatusCode, error: &str) -> Response {
    (status, Json(serde_json::json!({ "error": error }))).into_response()
}

fn random_peer_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "phx_peer_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
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

    fn resolution(
        kind: super::super::global_read::ResolvedReferenceKind,
        id: &str,
        title: Option<&str>,
        href: Option<&str>,
    ) -> super::super::global_read::ResolveGlobalReferenceResponse {
        super::super::global_read::ResolveGlobalReferenceResponse {
            kind,
            id: id.to_string(),
            title: title.map(str::to_string),
            href: href.map(str::to_string),
            work_scope: super::super::global_read::ResolvedWorkScope::Missing,
            summary: String::new(),
        }
    }

    #[test]
    fn peer_tokens_are_prefixed_random_credentials() {
        let first = random_peer_token();
        let second = random_peer_token();
        assert!(first.starts_with("phx_peer_"));
        assert_ne!(first, second);
        assert_eq!(first.len(), "phx_peer_".len() + 43);
    }

    #[tokio::test]
    async fn saved_peer_selection_accepts_uuid_or_unique_name_and_refuses_ambiguity() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let token = |byte: &str| {
            phoenix_core::domain::instance_identity::PeerBearerCredential::parse(format!(
                "phx_peer_{}",
                byte.repeat(43)
            ))
            .unwrap()
        };
        let first = InstanceId::new();
        let second = InstanceId::new();
        let shadow = InstanceId::new();
        let first_id = first.to_string();
        for (id, name, url) in [
            (first, "unique", "https://first.example"),
            (second, "duplicate", "https://second.example"),
            (shadow, first_id.as_str(), "https://shadow.example"),
        ] {
            state
                .db
                .save_federation_peer_connection(
                    id,
                    name,
                    &url.parse().unwrap(),
                    &token(if id == first {
                        "a"
                    } else if id == second {
                        "b"
                    } else {
                        "c"
                    }),
                    &PeerTlsTrust::PlatformRoots,
                )
                .await
                .unwrap();
        }
        assert_eq!(
            resolve_saved_peer_id(&state.db, &first.to_string())
                .await
                .unwrap(),
            first
        );
        assert_eq!(
            resolve_saved_peer_id(&state.db, "unique").await.unwrap(),
            first
        );
        state
            .db
            .save_federation_peer_connection(
                InstanceId::new(),
                "duplicate",
                &"https://other.example".parse().unwrap(),
                &token("d"),
                &PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();
        assert!(matches!(
            resolve_saved_peer_id(&state.db, "duplicate").await,
            Err(FederationClientError::PeerAmbiguous)
        ));
        assert!(matches!(
            resolve_saved_peer_id(&state.db, "missing").await,
            Err(FederationClientError::PeerNotFound)
        ));
    }

    #[tokio::test]
    async fn remote_read_rejects_oversized_cursor_before_peer_lookup() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;

        let error = read_remote_conversation(
            &state.db,
            "missing-peer",
            "@conv:missing-product",
            super::super::global_read::MAX_CONVERSATION_READ_CURSOR + 1,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, FederationClientError::RemoteRejected(message) if message.contains("cursor exceeds maximum"))
        );
    }

    #[test]
    fn remote_next_cursor_must_advance_within_the_local_cap() {
        assert!(validate_remote_next_cursor(0, None).is_ok());
        assert!(validate_remote_next_cursor(0, Some(1)).is_ok());
        assert!(validate_remote_next_cursor(1, Some(1)).is_err());
        assert!(validate_remote_next_cursor(2, Some(1)).is_err());
        assert!(validate_remote_next_cursor(
            0,
            Some(super::super::global_read::MAX_CONVERSATION_READ_CURSOR + 1),
        )
        .is_err());
    }

    #[tokio::test]
    async fn bounded_remote_read_rejects_exhausted_admission_and_deadline() {
        let exhausted = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        assert_eq!(
            execute_bounded_remote_read(&exhausted, std::time::Duration::from_secs(1), async { 1 })
                .await,
            Err(RemoteReadLimitError::Admission)
        );

        let admitted = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        assert_eq!(
            execute_bounded_remote_read(
                &admitted,
                std::time::Duration::ZERO,
                std::future::pending::<usize>(),
            )
            .await,
            Err(RemoteReadLimitError::Deadline)
        );
        assert_eq!(admitted.available_permits(), 1);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn peer_routes_enforce_auth_destination_and_stable_exact_semantics() {
        use tower::ServiceExt as _;

        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        state
            .db
            .create_conversation("remote-transcript", "remote", "/tmp", true, None, None)
            .await
            .unwrap();
        let opaque_transcript_id = "opaque/segment?x=1&state=50%";
        state
            .db
            .create_conversation(
                opaque_transcript_id,
                "opaque",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        for index in 0..180 {
            state
                .db
                .add_message(
                    &format!("remote-message-{index}"),
                    "remote-transcript",
                    &crate::db::MessageContent::user(format!(
                        "bounded remote message {index} {}",
                        "x".repeat(80)
                    )),
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        let product_id: String = sqlx::query_scalar(
            "SELECT product_conversation_id FROM conversations WHERE id = 'remote-transcript'",
        )
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        let caller = InstanceId::new();
        let bearer = format!("phx_peer_{}", "b".repeat(43));
        state
            .db
            .replace_federation_enrollment(
                caller,
                "caller",
                &FederationCredentialVerifier::from_bearer(bearer.as_bytes()),
            )
            .await
            .unwrap();
        let destination = state.db.instance_id().await.unwrap();
        let app = crate::api::handlers::create_router(state.clone());
        let request = |path: &str, body: serde_json::Value, auth: Option<&str>| {
            let mut builder = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json");
            if let Some(auth) = auth {
                builder = builder.header("authorization", format!("Bearer {auth}"));
            }
            builder
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        };

        let stable = serde_json::to_value(RemoteResolveReferenceRequest {
            destination_instance_id: destination,
            target: RemoteConversationTarget::ProductConversation {
                product_conversation_id: product_id.clone(),
            },
        })
        .unwrap();
        let unauthenticated = app
            .clone()
            .oneshot(request(
                "/api/federation/peer/resolve-reference",
                stable.clone(),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
        let resolved = app
            .clone()
            .oneshot(request(
                "/api/federation/peer/resolve-reference",
                stable,
                Some(&bearer),
            ))
            .await
            .unwrap();
        assert_eq!(resolved.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resolved.into_body(), MAX_REMOTE_RESPONSE_BYTES)
            .await
            .unwrap();
        let resolved: RemoteResolveReferenceResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(resolved.resolution.kind.as_str(), "product_conversation");
        assert_eq!(resolved.resolution.id, product_id);
        assert_eq!(resolved.resolution.title.as_deref(), Some("Remote"));

        let opaque = serde_json::to_value(RemoteResolveReferenceRequest {
            destination_instance_id: destination,
            target: RemoteConversationTarget::ExactTranscript {
                transcript_id: opaque_transcript_id.to_string(),
                message_id: None,
            },
        })
        .unwrap();
        let opaque = app
            .clone()
            .oneshot(request(
                "/api/federation/peer/resolve-reference",
                opaque,
                Some(&bearer),
            ))
            .await
            .unwrap();
        assert_eq!(opaque.status(), StatusCode::OK);
        let body = axum::body::to_bytes(opaque.into_body(), MAX_REMOTE_RESPONSE_BYTES)
            .await
            .unwrap();
        let opaque: RemoteResolveReferenceResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(opaque.resolution.id, opaque_transcript_id);
        let expected_opaque_href =
            super::super::global_read::exact_conversation_href(opaque_transcript_id, None);
        assert_eq!(
            opaque.resolution.href.as_deref(),
            Some(expected_opaque_href.as_str())
        );

        let exact = |cursor| {
            serde_json::to_value(RemoteReadConversationRequest {
                destination_instance_id: destination,
                target: RemoteConversationTarget::ExactTranscript {
                    transcript_id: "remote-transcript".to_string(),
                    message_id: None,
                },
                cursor,
            })
            .unwrap()
        };
        let first = app
            .clone()
            .oneshot(request(
                "/api/federation/peer/read-conversation",
                exact(0),
                Some(&bearer),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let body = axum::body::to_bytes(first.into_body(), MAX_REMOTE_RESPONSE_BYTES)
            .await
            .unwrap();
        let first: RemoteReadConversationResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(first.cursor, 0);
        assert_eq!(first.next_cursor, Some(7000));
        assert_eq!(
            first.requested,
            RemoteConversationTarget::ExactTranscript {
                transcript_id: "remote-transcript".to_string(),
                message_id: None,
            }
        );
        assert!(!first.content.contains("call read_conversation again"));
        let next_cursor = first.next_cursor.unwrap();
        let second = app
            .clone()
            .oneshot(request(
                "/api/federation/peer/read-conversation",
                exact(next_cursor),
                Some(&bearer),
            ))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let body = axum::body::to_bytes(second.into_body(), MAX_REMOTE_RESPONSE_BYTES)
            .await
            .unwrap();
        let second: RemoteReadConversationResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(second.cursor, next_cursor);
        assert_ne!(second.content, first.content);

        let wrong = serde_json::to_value(RemoteReadConversationRequest {
            destination_instance_id: InstanceId::new(),
            target: RemoteConversationTarget::ExactTranscript {
                transcript_id: "remote-transcript".to_string(),
                message_id: None,
            },
            cursor: 0,
        })
        .unwrap();
        let refused = app
            .oneshot(request(
                "/api/federation/peer/read-conversation",
                wrong,
                Some(&bearer),
            ))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn remote_provenance_rejects_duplicate_pins_and_delimiter_smuggling() {
        let exact = RemoteConversationTarget::ExactTranscript {
            transcript_id: "transcript".to_string(),
            message_id: None,
        };
        let duplicate = resolution(
            super::super::global_read::ResolvedReferenceKind::Conversation,
            "transcript",
            Some("Transcript"),
            Some("/c/transcript?source_transcript=other&source_transcript=transcript"),
        );
        assert!(validate_resolution_binding(&duplicate, &exact).is_err());
        let smuggled = RemoteConversationTarget::ExactTranscript {
            transcript_id: "transcript#message-message".to_string(),
            message_id: None,
        };
        assert!(require_bounded_read_target(&smuggled).is_err());
    }

    #[test]
    fn remote_provenance_preserves_opaque_exact_target_ids() {
        let transcript_id = "transcript/part?query&percent%";
        let message_id = "message/part?query&percent%";
        let target = RemoteConversationTarget::ExactTranscript {
            transcript_id: transcript_id.to_string(),
            message_id: Some(message_id.to_string()),
        };
        let href =
            super::super::global_read::exact_conversation_href(transcript_id, Some(message_id));
        let resolved = resolution(
            super::super::global_read::ResolvedReferenceKind::Message,
            message_id,
            Some("Transcript"),
            Some(&href),
        );

        assert_eq!(
            href,
            "/c/transcript%2Fpart%3Fquery%26percent%25?source_transcript=transcript%2Fpart%3Fquery%26percent%25#message-message%2Fpart%3Fquery%26percent%25"
        );
        assert!(validate_resolution_binding(&resolved, &target).is_ok());
    }

    #[test]
    fn remote_provenance_accepts_exact_messages_without_rendered_anchors() {
        let target = RemoteConversationTarget::ExactTranscript {
            transcript_id: "transcript".to_string(),
            message_id: Some("tool-message".to_string()),
        };
        let resolved = resolution(
            super::super::global_read::ResolvedReferenceKind::Message,
            "tool-message",
            Some("Transcript"),
            Some("/c/transcript?source_transcript=transcript"),
        );

        assert!(validate_resolution_binding(&resolved, &target).is_ok());
    }

    #[test]
    fn remote_provenance_rejects_wrong_exact_message_fragment() {
        let target = RemoteConversationTarget::ExactTranscript {
            transcript_id: "transcript".to_string(),
            message_id: Some("message".to_string()),
        };
        let wrong = resolution(
            super::super::global_read::ResolvedReferenceKind::Message,
            "message",
            Some("Transcript"),
            Some("/c/transcript?source_transcript=transcript#message-other"),
        );
        assert!(validate_resolution_binding(&wrong, &target).is_err());
        let exact = resolution(
            super::super::global_read::ResolvedReferenceKind::Message,
            "message",
            Some("Transcript"),
            Some("/c/transcript?source_transcript=transcript#message-message"),
        );
        assert!(validate_resolution_binding(&exact, &target).is_ok());
    }

    #[test]
    fn remote_provenance_rejects_non_relative_hrefs_for_stable_targets() {
        let target = RemoteConversationTarget::ProductConversation {
            product_conversation_id: "product".to_string(),
        };
        for href in [
            "https://attacker.invalid/product",
            "//attacker.invalid/product",
        ] {
            let resolved = resolution(
                super::super::global_read::ResolvedReferenceKind::ProductConversation,
                "product",
                Some("Product"),
                Some(href),
            );
            assert!(validate_resolution_binding(&resolved, &target).is_err());
        }
    }

    #[test]
    fn remote_wire_rejects_malformed_missing_and_unknown_fields() {
        let id = InstanceId::new();
        let null_content = serde_json::json!({
            "destination_instance_id": id,
            "caller_instance_id": id,
            "source_instance_id": id,
            "requested": {"kind": "product_conversation", "product_conversation_id": "product"},
            "cursor": 0,
            "next_cursor": null,
            "content": null,
        });
        assert!(serde_json::from_value::<RemoteReadConversationResponse>(null_content).is_err());

        let valid = serde_json::json!({
            "destination_instance_id": id,
            "caller_instance_id": id,
            "path_owner_instance_id": id,
            "requested": {"kind": "product_conversation", "product_conversation_id": "product"},
            "resolution": {
                "kind": "product_conversation",
                "id": "product",
                "title": "Product",
                "href": "/conversations/product",
                "work_scope": {"status": "missing"},
                "summary": ""
            }
        });
        assert!(
            serde_json::from_value::<RemoteResolveReferenceResponse>(valid.clone()).is_ok()
        );

        for field in ["kind", "id", "href", "title", "work_scope", "summary"] {
            let mut missing = valid.clone();
            missing["resolution"].as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<RemoteResolveReferenceResponse>(missing).is_err(),
                "missing resolution field was accepted: {field}"
            );
        }

        for (field, malformed) in [
            ("kind", serde_json::Value::Null),
            ("id", serde_json::Value::Null),
            ("work_scope", serde_json::json!({"status": "unknown"})),
            ("summary", serde_json::Value::Null),
        ] {
            let mut response = valid.clone();
            response["resolution"][field] = malformed;
            assert!(
                serde_json::from_value::<RemoteResolveReferenceResponse>(response).is_err(),
                "malformed resolution field was accepted: {field}"
            );
        }

        let mut unknown = valid;
        unknown["resolution"]["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<RemoteResolveReferenceResponse>(unknown).is_err());
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
    async fn outbound_resolve_uses_authenticated_https_and_validates_provenance() {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let caller = state.db.instance_id().await.unwrap();
        let destination = InstanceId::new();
        let bearer = phoenix_core::domain::instance_identity::PeerBearerCredential::parse(format!(
            "phx_peer_{}",
            "c".repeat(43)
        ))
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut tls = crate::tls::load_config(&crate::tls::ConfigSource::Auto {
            dir: temp.path().to_path_buf(),
            hosts: vec!["localhost".to_string()],
        })
        .unwrap();
        tls.server.alpn_protocols = vec![b"http/1.1".to_vec()];
        let trust = PeerTlsTrust::PrivateCa {
            certificate_pem: phoenix_core::domain::instance_identity::PeerCaCertificatePem::parse(
                std::fs::read_to_string(tls.ca_cert_path.unwrap()).unwrap(),
            )
            .unwrap(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        state
            .db
            .save_federation_peer_connection(
                destination,
                "remote",
                &format!("https://localhost:{port}").parse().unwrap(),
                &bearer,
                &trust,
            )
            .await
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(tls.server));
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let stream = acceptor.accept(stream).await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                stream.read_until(b'\n', &mut headers).await.unwrap();
            }
            let headers = String::from_utf8(headers).unwrap();
            assert!(headers.starts_with("POST /api/federation/peer/resolve-reference HTTP/1.1"));
            assert!(headers
                .to_ascii_lowercase()
                .contains("authorization: bearer phx_peer_"));
            let body = serde_json::to_vec(&RemoteResolveReferenceResponse {
                destination_instance_id: destination,
                caller_instance_id: caller,
                path_owner_instance_id: destination,
                requested: RemoteConversationTarget::ProductConversation {
                    product_conversation_id: "product".to_string(),
                },
                resolution: resolution(
                    super::super::global_read::ResolvedReferenceKind::ProductConversation,
                    "product",
                    Some("Product"),
                    Some("/conversations/product"),
                ),
            })
            .unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&body).await.unwrap();
        });

        resolve_remote_reference(&state.db, "remote", "@conv:product")
            .await
            .unwrap();
        server.await.unwrap();
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
