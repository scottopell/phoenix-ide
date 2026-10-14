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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", content = "work_scope_id", rename_all = "snake_case")]
pub enum ExpectedDestinationWorkScope {
    Work(phoenix_core::work_scope::WorkScopeId),
    Unattached,
}

impl ExpectedDestinationWorkScope {
    fn into_option(self) -> Option<phoenix_core::work_scope::WorkScopeId> {
        match self {
            Self::Work(work_scope_id) => Some(work_scope_id),
            Self::Unattached => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FederationOwnerContinueRequest {
    pub destination_instance_id: InstanceId,
    pub product_conversation_id: phoenix_core::domain::product_conversation::ProductConversationId,
    pub current_predecessor_conversation_id: String,
    pub expected_work_scope: ExpectedDestinationWorkScope,
    pub handoff: String,
    pub message_id: String,
    #[serde(default)]
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FederationOwnerIntentAdmission {
    Created,
    ExactRetry,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", content = "error", rename_all = "snake_case")]
pub enum FederationOwnerDispatchStatus {
    Accepted,
    AlreadySettled,
    Failed(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FederationOwnerContinueResponse {
    pub destination_instance_id: InstanceId,
    pub product_conversation_id: phoenix_core::domain::product_conversation::ProductConversationId,
    pub predecessor_conversation_id: String,
    pub successor_conversation_id: String,
    pub successor_slug: Option<String>,
    pub intent_admission: FederationOwnerIntentAdmission,
    pub dispatch: FederationOwnerDispatchStatus,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FederationPeerOwnerContinueRequest {
    pub caller_instance_id: InstanceId,
    pub request: FederationOwnerContinueRequest,
}

struct LocalOwnerContinueAuthority;

enum OwnerContinueAuthority {
    Owner(OwnerAuthenticated),
    Delegated(LocalOwnerContinueAuthority),
}

const OWNER_CONTROL_CALLER_ENV: &str = "PHOENIX_FEDERATION_OWNER_CONTROL_CALLER_INSTANCE_ID";
const OWNER_CREDENTIAL_PLIST_ENV: &str = "PHOENIX_FEDERATION_OWNER_CREDENTIAL_PLIST";
const MAX_OWNER_CREDENTIAL_SOURCE_BYTES: u64 = 64 * 1024;

fn configured_local_owner_control_authority(
    state: &AppState,
    authenticated_caller: InstanceId,
) -> Result<LocalOwnerContinueAuthority, FederationClientError> {
    let configured_caller = std::env::var(OWNER_CONTROL_CALLER_ENV)
        .map_err(|_| FederationClientError::OwnerCredentialUnavailable)?
        .parse::<InstanceId>()
        .map_err(|_| FederationClientError::OwnerCredentialConfiguration)?;
    let source = std::env::var_os(OWNER_CREDENTIAL_PLIST_ENV)
        .map(std::path::PathBuf::from)
        .ok_or(FederationClientError::OwnerCredentialUnavailable)?;
    local_owner_control_authority_from_source(
        configured_caller,
        authenticated_caller,
        state.password.as_ref(),
        &source,
    )
}

fn local_owner_control_authority_from_source(
    configured_caller: InstanceId,
    authenticated_caller: InstanceId,
    active_owner_password: Option<&phoenix_core::domain::instance_identity::OwnerPassword>,
    source: &std::path::Path,
) -> Result<LocalOwnerContinueAuthority, FederationClientError> {
    if configured_caller != authenticated_caller {
        return Err(FederationClientError::OwnerControlCallerMismatch);
    }
    let configured_password = read_owner_password_from_launch_agent(source)?;
    let active_owner_password =
        active_owner_password.ok_or(FederationClientError::OwnerCredentialUnavailable)?;
    let configured_verifier =
        phoenix_core::domain::instance_identity::FederationCredentialVerifier::from_bearer(
            configured_password.expose().as_bytes(),
        );
    let active_verifier =
        phoenix_core::domain::instance_identity::FederationCredentialVerifier::from_bearer(
            active_owner_password.expose().as_bytes(),
        );
    if configured_verifier != active_verifier {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    Ok(LocalOwnerContinueAuthority)
}

fn plist_dict_value<'a, 'input>(
    dictionary: roxmltree::Node<'a, 'input>,
    key_name: &str,
) -> Result<roxmltree::Node<'a, 'input>, FederationClientError> {
    if !dictionary.has_tag_name("dict") {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    let children: Vec<_> = dictionary
        .children()
        .filter(roxmltree::Node::is_element)
        .collect();
    let mut values = children.iter().enumerate().filter_map(|(index, node)| {
        (node.has_tag_name("key") && node.text() == Some(key_name))
            .then(|| children.get(index + 1).copied())
            .flatten()
    });
    let value = values
        .next()
        .ok_or(FederationClientError::OwnerCredentialUnavailable)?;
    if values.next().is_some() {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    Ok(value)
}

fn read_owner_password_from_launch_agent(
    source: &std::path::Path,
) -> Result<phoenix_core::domain::instance_identity::OwnerPassword, FederationClientError> {
    if !source.is_absolute() {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    let file = std::fs::File::open(source)
        .map_err(|_| FederationClientError::OwnerCredentialUnavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| FederationClientError::OwnerCredentialUnavailable)?;
    if !metadata.is_file() || metadata.len() > MAX_OWNER_CREDENTIAL_SOURCE_BYTES {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    let mut xml = String::new();
    std::io::Read::read_to_string(
        &mut std::io::Read::take(file, MAX_OWNER_CREDENTIAL_SOURCE_BYTES + 1),
        &mut xml,
    )
    .map_err(|_| FederationClientError::OwnerCredentialUnavailable)?;
    if xml.len() as u64 > MAX_OWNER_CREDENTIAL_SOURCE_BYTES {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    let document = roxmltree::Document::parse(&xml)
        .map_err(|_| FederationClientError::OwnerCredentialConfiguration)?;
    let plist = document.root_element();
    if !plist.has_tag_name("plist") {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    let mut root_values = plist.children().filter(roxmltree::Node::is_element);
    let root_dictionary = root_values
        .next()
        .ok_or(FederationClientError::OwnerCredentialConfiguration)?;
    if root_values.next().is_some() {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    let environment = plist_dict_value(root_dictionary, "EnvironmentVariables")?;
    let password = plist_dict_value(environment, "PHOENIX_PASSWORD")?;
    if !password.has_tag_name("string") {
        return Err(FederationClientError::OwnerCredentialConfiguration);
    }
    let value = password
        .text()
        .ok_or(FederationClientError::OwnerCredentialUnavailable)?;
    phoenix_core::domain::instance_identity::OwnerPassword::parse(value.to_string())
        .map_err(|_| FederationClientError::OwnerCredentialConfiguration)
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
    #[error("authenticated peer is not the configured owner-control caller")]
    OwnerControlCallerMismatch,
    #[error("destination owner credential capability is unavailable")]
    OwnerCredentialUnavailable,
    #[error("destination owner credential source configuration is invalid")]
    OwnerCredentialConfiguration,
    #[error("remote owner Continue rejected with {0}")]
    OwnerContinueRejected(StatusCode),
    #[error("remote owner Continue product identity mismatch")]
    ProductMismatch,
    #[error("remote owner Continue predecessor identity mismatch")]
    PredecessorMismatch,
}

pub async fn continue_remote_owner(
    db: &crate::db::Database,
    request: &FederationOwnerContinueRequest,
) -> Result<FederationOwnerContinueResponse, FederationClientError> {
    let peer = db
        .federation_peer_connection(request.destination_instance_id)
        .await?
        .ok_or(FederationClientError::PeerNotFound)?;
    let caller_instance_id = db.instance_id().await?;
    let endpoint = peer.base_url.peer_owner_continue_endpoint();
    let client = federation_client(&peer.tls_trust)?;
    let response = client
        .post(endpoint.as_url().clone())
        .bearer_auth(peer.bearer_credential.expose())
        .json(&FederationPeerOwnerContinueRequest {
            caller_instance_id,
            request: request.clone(),
        })
        .send()
        .await?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, MAX_REMOTE_RESPONSE_BYTES)?;
    }
    decode_remote_owner_continue_response(status, &bytes, request)
}

fn decode_remote_owner_continue_response(
    status: StatusCode,
    bytes: &[u8],
    request: &FederationOwnerContinueRequest,
) -> Result<FederationOwnerContinueResponse, FederationClientError> {
    if !status.is_success() {
        return Err(FederationClientError::OwnerContinueRejected(status));
    }
    let response: FederationOwnerContinueResponse = serde_json::from_slice(bytes)?;
    if response.destination_instance_id != request.destination_instance_id {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.product_conversation_id != request.product_conversation_id {
        return Err(FederationClientError::ProductMismatch);
    }
    if response.predecessor_conversation_id != request.current_predecessor_conversation_id {
        return Err(FederationClientError::PredecessorMismatch);
    }
    Ok(response)
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
    let client = federation_client(&peer.tls_trust)?;
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

fn federation_client(tls_trust: &PeerTlsTrust) -> Result<reqwest::Client, FederationClientError> {
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

fn random_peer_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "phx_peer_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

pub async fn forward_owner_continue(
    _owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<FederationOwnerContinueRequest>,
) -> Result<Json<FederationOwnerContinueResponse>, Response> {
    continue_remote_owner(&state.db, &request)
        .await
        .map(Json)
        .map_err(|error| map_owner_continue_client_error(&error))
}

fn owner_continue_error_response(status: StatusCode, error_type: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": message,
            "error_type": error_type,
        })),
    )
        .into_response()
}

fn map_owner_continue_client_error(error: &FederationClientError) -> Response {
    match error {
        FederationClientError::OwnerCredentialUnavailable => owner_continue_error_response(
            StatusCode::FAILED_DEPENDENCY,
            "destination_owner_credential_unavailable",
            "destination owner credential capability is unavailable",
        ),
        FederationClientError::OwnerCredentialConfiguration => owner_continue_error_response(
            StatusCode::FAILED_DEPENDENCY,
            "destination_owner_credential_configuration_invalid",
            "destination owner credential source configuration is invalid",
        ),
        FederationClientError::PeerNotFound => owner_continue_error_response(
            StatusCode::FAILED_DEPENDENCY,
            "federation_destination_unavailable",
            "selected destination peer connection is unavailable",
        ),
        FederationClientError::OwnerContinueRejected(status) => {
            let (forwarded_status, error_type, message) = match *status {
                StatusCode::BAD_REQUEST => (
                    StatusCode::BAD_REQUEST,
                    "remote_owner_continue_bad_request",
                    "destination owner Continue rejected the request",
                ),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => (
                    StatusCode::FORBIDDEN,
                    "destination_owner_auth_rejected",
                    "destination owner authentication was rejected",
                ),
                StatusCode::NOT_FOUND => (
                    StatusCode::NOT_FOUND,
                    "remote_owner_continue_target_not_found",
                    "destination owner Continue target was not found",
                ),
                StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY => (
                    *status,
                    "remote_owner_continue_rejected",
                    "destination owner Continue rejected the request",
                ),
                StatusCode::FAILED_DEPENDENCY => (
                    StatusCode::FAILED_DEPENDENCY,
                    "destination_owner_capability_unavailable",
                    "destination owner-control capability is unavailable",
                ),
                StatusCode::TOO_MANY_REQUESTS => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "destination_owner_throttled",
                    "destination owner Continue is throttled",
                ),
                StatusCode::SERVICE_UNAVAILABLE => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "destination_owner_unavailable",
                    "destination owner Continue is unavailable",
                ),
                _ => (
                    StatusCode::BAD_GATEWAY,
                    "federation_owner_continue_transport_failed",
                    "destination owner Continue returned an unexpected status",
                ),
            };
            owner_continue_error_response(forwarded_status, error_type, message)
        }
        FederationClientError::OwnerControlCallerMismatch
        | FederationClientError::DestinationMismatch
        | FederationClientError::ProductMismatch
        | FederationClientError::PredecessorMismatch => owner_continue_error_response(
            StatusCode::CONFLICT,
            "federation_destination_mismatch",
            "federation owner Continue destination identity mismatch",
        ),
        FederationClientError::Request(_)
        | FederationClientError::ResponseTooLarge(_)
        | FederationClientError::RemoteRejected(_)
        | FederationClientError::InvalidResponse(_)
        | FederationClientError::CallerMismatch => owner_continue_error_response(
            StatusCode::BAD_GATEWAY,
            "federation_owner_continue_transport_failed",
            "federation owner Continue transport failed",
        ),
        FederationClientError::Database(_) => owner_continue_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "federation_owner_continue_configuration_failed",
            "federation owner Continue configuration lookup failed",
        ),
    }
}

fn map_local_owner_control_authority_error(error: &FederationClientError) -> Response {
    match error {
        FederationClientError::OwnerControlCallerMismatch => owner_continue_error_response(
            StatusCode::FORBIDDEN,
            "federation_owner_control_caller_not_allowed",
            "authenticated peer is not allowed to invoke owner Continue",
        ),
        FederationClientError::OwnerCredentialUnavailable => owner_continue_error_response(
            StatusCode::FAILED_DEPENDENCY,
            "destination_owner_credential_unavailable",
            "destination owner credential capability is unavailable",
        ),
        FederationClientError::OwnerCredentialConfiguration => owner_continue_error_response(
            StatusCode::FAILED_DEPENDENCY,
            "destination_owner_credential_configuration_invalid",
            "destination owner credential source configuration is invalid",
        ),
        FederationClientError::PeerNotFound
        | FederationClientError::Database(_)
        | FederationClientError::Request(_)
        | FederationClientError::ResponseTooLarge(_)
        | FederationClientError::RemoteRejected(_)
        | FederationClientError::InvalidResponse(_)
        | FederationClientError::DestinationMismatch
        | FederationClientError::CallerMismatch
        | FederationClientError::OwnerContinueRejected(_)
        | FederationClientError::ProductMismatch
        | FederationClientError::PredecessorMismatch => owner_continue_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "federation_owner_control_authority_failed",
            "destination owner-control authority check failed",
        ),
    }
}

pub async fn continue_owner(
    owner: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<FederationOwnerContinueRequest>,
) -> Result<Json<FederationOwnerContinueResponse>, super::handlers::AppError> {
    continue_owner_locally(&state, request, OwnerContinueAuthority::Owner(owner)).await
}

pub async fn continue_owner_from_peer(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(envelope): Json<FederationPeerOwnerContinueRequest>,
) -> Result<Json<FederationOwnerContinueResponse>, Response> {
    if envelope.caller_instance_id != peer.caller_instance_id {
        return Err(owner_continue_error_response(
            StatusCode::FORBIDDEN,
            "federation_owner_control_caller_mismatch",
            "authenticated peer does not match the declared owner-control caller",
        ));
    }
    let authority = configured_local_owner_control_authority(&state, peer.caller_instance_id)
        .map_err(|error| map_local_owner_control_authority_error(&error))?;
    continue_owner_locally(
        &state,
        envelope.request,
        OwnerContinueAuthority::Delegated(authority),
    )
    .await
    .map_err(IntoResponse::into_response)
}

async fn continue_owner_locally(
    state: &AppState,
    request: FederationOwnerContinueRequest,
    _authority: OwnerContinueAuthority,
) -> Result<Json<FederationOwnerContinueResponse>, super::handlers::AppError> {
    let local_instance_id = state
        .db
        .instance_id()
        .await
        .map_err(|error| super::handlers::AppError::Internal(error.to_string()))?;
    if request.destination_instance_id != local_instance_id {
        return Err(super::handlers::AppError::Conflict(Box::new(
            super::types::ConflictErrorResponse::new(
                "owner Continue request targets a different Phoenix instance",
                "destination_instance_mismatch",
            ),
        )));
    }
    let message_id =
        phoenix_workflow::ClientTurnKey::new(request.message_id.clone()).ok_or_else(|| {
            super::handlers::AppError::BadRequest(
                "Continuation message_id must not be empty.".to_string(),
            )
        })?;
    let product_conversation_id = request.product_conversation_id.clone();
    let predecessor_conversation_id = request.current_predecessor_conversation_id.clone();
    let result =
        crate::continuation_service::ContinuationApplicationService::new(state.runtime.clone())
            .continue_owner(crate::continuation_service::OwnerContinuationRequest {
                destination_product_conversation_id: request.product_conversation_id,
                predecessor_conversation_id: request.current_predecessor_conversation_id,
                expected_work_scope_id: request.expected_work_scope.into_option(),
                message_id,
                handoff: request.handoff,
                user_agent: request.user_agent,
            })
            .await
            .map_err(map_owner_continue_error)?;
    let intent_admission = match result.intent_admission {
        crate::continuation_service::OwnerContinuationIntentAdmission::Created => {
            FederationOwnerIntentAdmission::Created
        }
        crate::continuation_service::OwnerContinuationIntentAdmission::ExactRetry => {
            FederationOwnerIntentAdmission::ExactRetry
        }
    };
    let dispatch = match result.dispatch {
        crate::continuation_service::ContinuationDispatchStatus::Accepted => {
            FederationOwnerDispatchStatus::Accepted
        }
        crate::continuation_service::ContinuationDispatchStatus::AlreadySettled => {
            FederationOwnerDispatchStatus::AlreadySettled
        }
        crate::continuation_service::ContinuationDispatchStatus::Failed(error) => {
            FederationOwnerDispatchStatus::Failed(error)
        }
    };
    Ok(Json(FederationOwnerContinueResponse {
        destination_instance_id: local_instance_id,
        product_conversation_id,
        predecessor_conversation_id,
        successor_conversation_id: result.successor_conversation_id,
        successor_slug: result.successor_slug,
        intent_admission,
        dispatch,
    }))
}

fn map_owner_continue_error(
    error: crate::continuation_service::OwnerContinuationError,
) -> super::handlers::AppError {
    use crate::continuation_service::OwnerContinuationError;
    use phoenix_db::DbError;

    let detail = error.to_string();
    match error {
        OwnerContinuationError::EmptyHandoff => super::handlers::AppError::BadRequest(detail),
        OwnerContinuationError::IntentConflict => super::handlers::AppError::Conflict(Box::new(
            super::types::ConflictErrorResponse::new(detail, "continuation_intent_conflict"),
        )),
        OwnerContinuationError::PredecessorNotContextExhausted(_)
        | OwnerContinuationError::Database(DbError::ContinuationPrecondition(_)) => {
            super::handlers::AppError::Conflict(Box::new(super::types::ConflictErrorResponse::new(
                detail,
                "continuation_target_stale",
            )))
        }
        OwnerContinuationError::Database(DbError::ConversationNotFound(message)) => {
            super::handlers::AppError::NotFound(message)
        }
        OwnerContinuationError::Database(DbError::CloseAdmissionFenced(_)) => {
            super::handlers::AppError::Conflict(Box::new(super::types::ConflictErrorResponse::new(
                "continuation is fenced by Close",
                "continuation_not_allowed",
            )))
        }
        OwnerContinuationError::Database(DbError::ProductConversationUnavailable(product_id)) => {
            super::handlers::AppError::Conflict(Box::new(super::types::ConflictErrorResponse::new(
                format!("ProductConversation {product_id} is unavailable"),
                "continuation_not_allowed",
            )))
        }
        OwnerContinuationError::MissingIntent | OwnerContinuationError::Database(_) => {
            super::handlers::AppError::Internal(detail)
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

    #[test]
    fn owner_credential_source_authorizes_only_the_configured_peer_caller() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("owner.plist");
        std::fs::write(
            &source,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.phoenix-ide.server</string>
<key>EnvironmentVariables</key><dict>
<key>PHOENIX_PASSWORD</key><string>fixture-owner-password</string>
</dict></dict></plist>"#,
        )
        .unwrap();
        let configured_caller = InstanceId::new();
        let active_password = phoenix_core::domain::instance_identity::OwnerPassword::parse(
            "fixture-owner-password".to_string(),
        )
        .unwrap();
        assert!(local_owner_control_authority_from_source(
            configured_caller,
            configured_caller,
            Some(&active_password),
            &source,
        )
        .is_ok());

        let wrong_caller = local_owner_control_authority_from_source(
            configured_caller,
            InstanceId::new(),
            Some(&active_password),
            &source,
        );
        assert!(matches!(
            wrong_caller,
            Err(FederationClientError::OwnerControlCallerMismatch)
        ));
    }

    #[test]
    fn owner_credential_source_rejects_password_outside_environment_variables() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("deceptive.plist");
        std::fs::write(
            &source,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Unrelated</key><dict>
<key>PHOENIX_PASSWORD</key><string>not-the-launch-agent-password</string>
</dict>
</dict></plist>"#,
        )
        .unwrap();
        let result = read_owner_password_from_launch_agent(&source);
        assert!(matches!(
            result,
            Err(FederationClientError::OwnerCredentialUnavailable)
        ));
    }

    #[test]
    fn owner_continue_forwarding_preserves_actionable_failure_classes() {
        assert_eq!(
            map_owner_continue_client_error(&FederationClientError::OwnerCredentialUnavailable)
                .status(),
            StatusCode::FAILED_DEPENDENCY
        );
        assert_eq!(
            map_owner_continue_client_error(&FederationClientError::OwnerContinueRejected(
                StatusCode::TOO_MANY_REQUESTS,
            ))
            .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            map_owner_continue_client_error(&FederationClientError::OwnerContinueRejected(
                StatusCode::SERVICE_UNAVAILABLE,
            ))
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn remote_owner_error_body_is_never_reflected() {
        let destination = InstanceId::new();
        let request = FederationOwnerContinueRequest {
            destination_instance_id: destination,
            product_conversation_id:
                phoenix_core::domain::product_conversation::ProductConversationId::new(),
            current_predecessor_conversation_id: "predecessor".to_string(),
            expected_work_scope: ExpectedDestinationWorkScope::Unattached,
            handoff: "continue".to_string(),
            message_id: "reflected-error-message".to_string(),
            user_agent: None,
        };
        let secret = "fixture-owner-password-that-must-not-be-reflected";
        let body = serde_json::to_vec(&serde_json::json!({
            "error": format!("Authorization: Bearer {secret}")
        }))
        .unwrap();
        let error =
            decode_remote_owner_continue_response(StatusCode::UNAUTHORIZED, &body, &request)
                .unwrap_err();
        assert!(matches!(
            error,
            FederationClientError::OwnerContinueRejected(StatusCode::UNAUTHORIZED)
        ));
        assert!(!error.to_string().contains(secret));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn owner_continue_uses_selected_https_peer_and_local_owner_delegation() {
        use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let destination = InstanceId::new();
        let product = phoenix_core::domain::product_conversation::ProductConversationId::new();
        let temp = tempfile::tempdir().unwrap();
        let mut loaded = crate::tls::load_config(&crate::tls::ConfigSource::Auto {
            dir: temp.path().to_path_buf(),
            hosts: vec!["localhost".to_string()],
        })
        .unwrap();
        loaded.server.alpn_protocols = vec![b"http/1.1".to_vec()];
        let certificate_pem = phoenix_core::domain::instance_identity::PeerCaCertificatePem::parse(
            std::fs::read_to_string(loaded.ca_cert_path.unwrap()).unwrap(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        state
            .db
            .save_federation_peer_connection(
                destination,
                "destination",
                &format!("https://localhost:{}", address.port())
                    .parse()
                    .unwrap(),
                &phoenix_core::domain::instance_identity::PeerBearerCredential::parse(format!(
                    "phx_peer_{}",
                    "a".repeat(43)
                ))
                .unwrap(),
                &PeerTlsTrust::PrivateCa { certificate_pem },
            )
            .await
            .unwrap();
        let expected_destination = destination;
        let expected_product = product.clone();
        let expected_caller = state.db.instance_id().await.unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(loaded.server));
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let stream = acceptor.accept(stream).await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            let mut headers = Vec::new();
            stream.read_until(b'\n', &mut headers).await.unwrap();
            while !headers.ends_with(b"\r\n\r\n") {
                stream.read_until(b'\n', &mut headers).await.unwrap();
            }
            let headers = String::from_utf8(headers).unwrap();
            assert!(headers.starts_with("POST /api/federation/peer/owner-continue "));
            assert!(headers.to_ascii_lowercase().contains(&format!(
                "authorization: bearer phx_peer_{}\r\n",
                "a".repeat(43)
            )));
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap();
            let mut body = vec![0; content_length];
            stream.read_exact(&mut body).await.unwrap();
            let envelope: FederationPeerOwnerContinueRequest =
                serde_json::from_slice(&body).unwrap();
            assert_eq!(envelope.caller_instance_id, expected_caller);
            let request = envelope.request;
            assert_eq!(request.destination_instance_id, expected_destination);
            assert_eq!(request.product_conversation_id, expected_product);
            let response = serde_json::to_vec(&FederationOwnerContinueResponse {
                destination_instance_id: expected_destination,
                product_conversation_id: expected_product,
                predecessor_conversation_id: request.current_predecessor_conversation_id,
                successor_conversation_id: "successor".to_string(),
                successor_slug: Some("successor".to_string()),
                intent_admission: FederationOwnerIntentAdmission::Created,
                dispatch: FederationOwnerDispatchStatus::Accepted,
            })
            .unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        response.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&response).await.unwrap();
        });

        let request = FederationOwnerContinueRequest {
            destination_instance_id: destination,
            product_conversation_id: product,
            current_predecessor_conversation_id: "predecessor".to_string(),
            expected_work_scope: ExpectedDestinationWorkScope::Unattached,
            handoff: "continue".to_string(),
            message_id: "two-instance-message".to_string(),
            user_agent: None,
        };
        let response = continue_remote_owner(&state.db, &request).await.unwrap();
        assert_eq!(response.successor_conversation_id, "successor");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn owner_continue_never_falls_back_to_local_when_destination_is_unconfigured() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let destination = InstanceId::new();
        let request = FederationOwnerContinueRequest {
            destination_instance_id: destination,
            product_conversation_id:
                phoenix_core::domain::product_conversation::ProductConversationId::new(),
            current_predecessor_conversation_id: "remote-only".to_string(),
            expected_work_scope: ExpectedDestinationWorkScope::Unattached,
            handoff: "continue".to_string(),
            message_id: "missing-peer-message".to_string(),
            user_agent: None,
        };
        let error = continue_remote_owner(&state.db, &request)
            .await
            .unwrap_err();
        assert!(matches!(error, FederationClientError::PeerNotFound));
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
        let client = federation_client(&PeerTlsTrust::PrivateCa { certificate_pem }).unwrap();

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
