pub(crate) mod subscriptions;
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

pub(crate) const MAX_REMOTE_MESSAGE_BYTES: usize = 32 * 1024;
pub(crate) const MAX_REMOTE_SEND_REQUEST_BODY_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RemoteMessageText(String);

impl RemoteMessageText {
    pub fn parse(text: String) -> Result<Self, &'static str> {
        if text.trim().is_empty() || text.len() > MAX_REMOTE_MESSAGE_BYTES {
            return Err("message must be nonblank and at most 32768 UTF-8 bytes");
        }
        Ok(Self(text))
    }
}

impl<'de> Deserialize<'de> for RemoteMessageText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteSendTarget {
    ProductConversation {
        product_conversation_id: phoenix_core::domain::product_conversation::ProductConversationId,
    },
    Transcript {
        transcript_id: phoenix_core::domain::close::TranscriptConversationId,
    },
    Coordinator {},
}

impl RemoteSendTarget {
    pub(crate) fn parse_tool_target(raw: &str) -> Result<Self, String> {
        if raw == "@coordinator" {
            return Ok(Self::Coordinator {});
        }
        match crate::api::global_read::GlobalReadService::parse_read_target(raw)? {
            ConversationReferenceTarget::ProductConversation {
                product_conversation_id,
            } => Ok(Self::ProductConversation {
                product_conversation_id,
            }),
            ConversationReferenceTarget::Transcript { transcript_id } => {
                Ok(Self::Transcript { transcript_id })
            }
            ConversationReferenceTarget::Coordinator => Ok(Self::Coordinator {}),
            ConversationReferenceTarget::Message { .. } => {
                Err("send targets cannot be message citations".into())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSourceToolCall {
    message_id: RemoteSenderLocator,
    tool_use_id: RemoteSenderLocator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct RemoteSenderLocator(String);

impl RemoteSenderLocator {
    fn parse(value: String) -> Result<Self, &'static str> {
        if value.trim().is_empty() || value.len() > 1024 {
            return Err("sender locator must be nonblank and at most 1024 bytes");
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for RemoteSenderLocator {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteSenderAttestation {
    RemoteApi {},
    Conversation {
        product_conversation_id: phoenix_core::domain::product_conversation::ProductConversationId,
        transcript_id: phoenix_core::domain::close::TranscriptConversationId,
        source_call: Option<RemoteSourceToolCall>,
    },
    Coordinator {
        product_conversation_id: phoenix_core::domain::product_conversation::ProductConversationId,
        transcript_id: phoenix_core::domain::close::TranscriptConversationId,
        source_call: Option<RemoteSourceToolCall>,
    },
}

impl RemoteSenderAttestation {
    pub(crate) fn from_conversation(
        source: crate::db::Conversation,
        call: phoenix_core::domain::db_schema::SourceToolCall,
    ) -> Result<Self, String> {
        let call = RemoteSourceToolCall {
            message_id: RemoteSenderLocator::parse(call.message_id).map_err(str::to_string)?,
            tool_use_id: RemoteSenderLocator::parse(call.tool_use_id).map_err(str::to_string)?,
        };
        let transcript_id = phoenix_core::domain::close::TranscriptConversationId::parse(source.id)
            .map_err(|error| error.to_string())?;
        let product_conversation_id = source.product_conversation_id;
        Ok(
            if source.runtime_role == crate::work_scope::RuntimeRole::Coordinator {
                Self::Coordinator {
                    product_conversation_id,
                    transcript_id,
                    source_call: Some(call),
                }
            } else {
                Self::Conversation {
                    product_conversation_id,
                    transcript_id,
                    source_call: Some(call),
                }
            },
        )
    }

    fn into_origin(self, instance_id: InstanceId) -> phoenix_core::domain::db_schema::InputOrigin {
        use phoenix_core::domain::db_schema::{InputOrigin, SourceToolCall};
        let convert_call = |call: Option<RemoteSourceToolCall>| {
            call.map(|call| {
                Box::new(SourceToolCall {
                    message_id: call.message_id.0,
                    tool_use_id: call.tool_use_id.0,
                })
            })
        };
        match self {
            Self::RemoteApi {} => InputOrigin::RemoteApi { instance_id },
            Self::Conversation {
                product_conversation_id,
                transcript_id,
                source_call,
            } => InputOrigin::RemoteConversation {
                instance_id,
                product_conversation_id,
                transcript_id: transcript_id.to_string(),
                source_call: convert_call(source_call),
            },
            Self::Coordinator {
                product_conversation_id,
                transcript_id,
                source_call,
            } => InputOrigin::RemoteCoordinator {
                instance_id,
                product_conversation_id,
                transcript_id: transcript_id.to_string(),
                source_call: convert_call(source_call),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSendMessageRequest {
    pub destination_instance_id: InstanceId,
    pub target: RemoteSendTarget,
    pub message_id: uuid::Uuid,
    pub message: RemoteMessageText,
    pub sender: RemoteSenderAttestation,
}

impl RemoteSendMessageRequest {
    fn locators_are_bounded(&self) -> bool {
        let target_bounded = match &self.target {
            RemoteSendTarget::ProductConversation {
                product_conversation_id,
            } => product_conversation_id.as_str().len() <= 1024,
            RemoteSendTarget::Transcript { transcript_id } => transcript_id.as_str().len() <= 1024,
            RemoteSendTarget::Coordinator {} => true,
        };
        let sender_bounded = match &self.sender {
            RemoteSenderAttestation::RemoteApi {} => true,
            RemoteSenderAttestation::Conversation {
                product_conversation_id,
                transcript_id,
                ..
            }
            | RemoteSenderAttestation::Coordinator {
                product_conversation_id,
                transcript_id,
                ..
            } => {
                product_conversation_id.as_str().len() <= 1024
                    && transcript_id.as_str().len() <= 1024
            }
        };
        target_bounded && sender_bounded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSendDisposition {
    Delivered,
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteStableDestination {
    pub instance_id: InstanceId,
    pub product_conversation_id: phoenix_core::domain::product_conversation::ProductConversationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteSendNavigation {
    AppLocal { path: String },
    Peer { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteMessageAcceptance {
    pub accepting_transcript: phoenix_core::domain::close::TranscriptConversationId,
    pub stable_destination: RemoteStableDestination,
    pub stable_navigation: RemoteSendNavigation,
    pub accepting_transcript_navigation: RemoteSendNavigation,
    pub accepting_message_navigation: RemoteSendNavigation,
    pub disposition: RemoteSendDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSendRejectionCode {
    DestinationMismatch,
    InvalidTarget,
    SenderNotAllowed,
    TargetNotFound,
    IdempotencyConflict,
    TargetUnavailable,
    AdmissionRejected,
    Busy,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteSendReceipt {
    Accepted {
        acceptance: RemoteMessageAcceptance,
    },
    Duplicate {
        acceptance: RemoteMessageAcceptance,
    },
    Rejected {
        code: RemoteSendRejectionCode,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSendMessageResponse {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub target: RemoteSendTarget,
    pub message_id: uuid::Uuid,
    pub receipt: RemoteSendReceipt,
}

pub async fn send_conversation_message(
    authenticated: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteSendMessageRequest>,
) -> Response {
    if !request.locators_are_bounded() {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    let destination = match state.db.instance_id().await {
        Ok(destination) => destination,
        Err(error) => {
            tracing::error!(%error, "failed to load federation send destination");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let receipt = if request.destination_instance_id == destination {
        admit_remote_message(&state, authenticated.caller_instance_id, &request).await
    } else {
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::DestinationMismatch,
            message: "destination instance identity mismatch".into(),
        }
    };
    Json(RemoteSendMessageResponse {
        destination_instance_id: destination,
        caller_instance_id: authenticated.caller_instance_id,
        target: request.target,
        message_id: request.message_id,
        receipt,
    })
    .into_response()
}

async fn resolve_remote_send_target(
    state: &AppState,
    target: &RemoteSendTarget,
    sender: &RemoteSenderAttestation,
) -> Result<crate::send_chat_service::SendChatTarget, RemoteSendReceipt> {
    use crate::send_chat_service::SendChatTarget;
    let rejected = |code, message: String| RemoteSendReceipt::Rejected {
        code,
        message: bounded_send_diagnostic(&message),
    };
    let exact = match target {
        RemoteSendTarget::Coordinator {} => Some(
            state
                .db
                .coordinator_conversation_id()
                .await
                .map_err(|error| rejected(RemoteSendRejectionCode::Internal, error.to_string()))?
                .ok_or_else(|| {
                    rejected(
                        RemoteSendRejectionCode::TargetNotFound,
                        "Coordinator unavailable".into(),
                    )
                })?,
        ),
        RemoteSendTarget::Transcript { transcript_id } => Some(transcript_id.to_string()),
        RemoteSendTarget::ProductConversation { .. } => None,
    };
    if let Some(id) = &exact {
        let conversation = state.db.get_conversation(id).await.map_err(|error| {
            rejected(RemoteSendRejectionCode::TargetNotFound, error.to_string())
        })?;
        if conversation.parent_conversation_id.is_some() {
            return Err(rejected(
                RemoteSendRejectionCode::InvalidTarget,
                "sub-agent targets are not allowed".into(),
            ));
        }
        if conversation.runtime_role == crate::work_scope::RuntimeRole::Coordinator {
            if !matches!(sender, RemoteSenderAttestation::Coordinator { .. }) {
                return Err(rejected(
                    RemoteSendRejectionCode::SenderNotAllowed,
                    "only a remote Coordinator may message the destination Coordinator".into(),
                ));
            }
            return Ok(if matches!(target, RemoteSendTarget::Coordinator {}) {
                SendChatTarget::StableCoordinator(conversation.product_conversation_id)
            } else {
                SendChatTarget::ExactTranscript(id.clone())
            });
        }
    }
    let raw = match target {
        RemoteSendTarget::ProductConversation {
            product_conversation_id,
        } => format!("@conv:{product_conversation_id}"),
        RemoteSendTarget::Transcript { transcript_id } => format!("@transcript:{transcript_id}"),
        RemoteSendTarget::Coordinator {} => {
            return Err(rejected(
                RemoteSendRejectionCode::InvalidTarget,
                "Coordinator role unavailable".into(),
            ))
        }
    };
    let service = crate::api::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    match service
        .resolve_message_target(&raw)
        .await
        .map_err(|error| rejected(RemoteSendRejectionCode::InvalidTarget, error.to_string()))?
    {
        crate::api::global_read::GlobalMessageTarget::StableProductConversation {
            product_conversation_id,
        } => Ok(SendChatTarget::StableProductConversation(
            product_conversation_id.to_string(),
        )),
        crate::api::global_read::GlobalMessageTarget::ExactTranscript { transcript_id } => {
            Ok(SendChatTarget::ExactTranscript(transcript_id.to_string()))
        }
    }
}

async fn admit_remote_message(
    state: &AppState,
    caller: InstanceId,
    request: &RemoteSendMessageRequest,
) -> RemoteSendReceipt {
    use crate::send_chat_service::{
        MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatReplay,
        SendChatRequest,
    };
    let target = match resolve_remote_send_target(state, &request.target, &request.sender).await {
        Ok(target) => target,
        Err(receipt) => return receipt,
    };
    let service = SendChatApplicationService::new(state.db.clone(), state.runtime.clone());
    let admission = service
        .send_to_target_with_replay(
            target,
            SendChatRequest {
                conversation_id: String::new(),
                origin: request.sender.clone().into_origin(caller),
                text: request.message.0.clone(),
                message_id: request.message_id.to_string(),
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                expansion_policy: MessageExpansionPolicy::LiteralText,
            },
        )
        .await;
    let (id, outcome, replay) = match admission {
        Ok(result) => result,
        Err(error) => return remote_send_admission_rejection(&error),
    };
    if let SendChatOutcome::Rejected { message, code } = outcome {
        return remote_send_outcome_rejection(&message, code);
    }
    let conversation = match state.db.get_conversation(&id).await {
        Ok(conversation) => conversation,
        Err(error) => {
            return RemoteSendReceipt::Rejected {
                code: RemoteSendRejectionCode::Internal,
                message: bounded_send_diagnostic(&error.to_string()),
            }
        }
    };
    let disposition =
        if replay == SendChatReplay::Steering || outcome == SendChatOutcome::QueuedAsSteering {
            RemoteSendDisposition::Queued
        } else {
            RemoteSendDisposition::Delivered
        };
    let stable_path = if conversation.runtime_role == crate::work_scope::RuntimeRole::Coordinator {
        "/global".to_string()
    } else {
        match state
            .db
            .ordinary_product_conversation_citation(&conversation.product_conversation_id)
            .await
        {
            Ok(Some(citation)) => super::product_conversations::product_route(
                citation.root_slug.as_deref(),
                citation.product_conversation_id.as_str(),
            ),
            Ok(None) | Err(_) => {
                return RemoteSendReceipt::Rejected {
                    code: RemoteSendRejectionCode::Internal,
                    message: "accepted destination navigation unavailable".into(),
                }
            }
        }
    };
    let transcript = phoenix_core::domain::close::TranscriptConversationId::parse(id)
        .expect("persisted transcript is nonempty");
    let exact_path = super::product_conversations::exact_transcript_route(&transcript);
    let message_id = accepting_message_id(&transcript, request.message_id, disposition);
    let acceptance = RemoteMessageAcceptance {
        accepting_transcript: transcript,
        stable_navigation: RemoteSendNavigation::AppLocal { path: stable_path },
        accepting_transcript_navigation: RemoteSendNavigation::AppLocal {
            path: exact_path.clone(),
        },
        accepting_message_navigation: RemoteSendNavigation::AppLocal {
            path: format!(
                "{exact_path}{}",
                super::product_conversations::message_fragment(&message_id)
            ),
        },
        stable_destination: RemoteStableDestination {
            instance_id: request.destination_instance_id,
            product_conversation_id: conversation.product_conversation_id,
        },
        disposition,
    };
    if replay == SendChatReplay::New {
        RemoteSendReceipt::Accepted { acceptance }
    } else {
        RemoteSendReceipt::Duplicate { acceptance }
    }
}

fn accepting_message_id(
    transcript: &phoenix_core::domain::close::TranscriptConversationId,
    message_id: uuid::Uuid,
    disposition: RemoteSendDisposition,
) -> String {
    match disposition {
        RemoteSendDisposition::Delivered => format!("{transcript}:{message_id}"),
        RemoteSendDisposition::Queued => message_id.to_string(),
    }
}

fn remote_send_admission_rejection(
    error: &crate::send_chat_service::SendChatServiceError,
) -> RemoteSendReceipt {
    use crate::send_chat_service::SendChatServiceError;
    let code = match error {
        SendChatServiceError::IdempotencyConflict => RemoteSendRejectionCode::IdempotencyConflict,
        SendChatServiceError::NotFound(_) => RemoteSendRejectionCode::TargetNotFound,
        SendChatServiceError::Busy => RemoteSendRejectionCode::Busy,
        SendChatServiceError::CloseAdmissionFenced | SendChatServiceError::HistoryUnavailable => {
            RemoteSendRejectionCode::TargetUnavailable
        }
        SendChatServiceError::AttachmentValidation(_) | SendChatServiceError::Expansion { .. } => {
            RemoteSendRejectionCode::AdmissionRejected
        }
        SendChatServiceError::Internal(_) | SendChatServiceError::Dispatch(_) => {
            RemoteSendRejectionCode::Internal
        }
    };
    RemoteSendReceipt::Rejected {
        code,
        message: bounded_send_diagnostic(&error.to_string()),
    }
}

fn remote_send_outcome_rejection(message: &str, code: &str) -> RemoteSendReceipt {
    RemoteSendReceipt::Rejected {
        code: match code {
            "target_unavailable" => RemoteSendRejectionCode::TargetUnavailable,
            _ => RemoteSendRejectionCode::AdmissionRejected,
        },
        message: bounded_send_diagnostic(message),
    }
}

fn bounded_send_diagnostic(message: &str) -> String {
    message.chars().take(512).collect()
}

pub async fn send_remote_conversation_message(
    local_db: &crate::db::Database,
    peer: &phoenix_db::FederationPeerConnection,
    request: &RemoteSendMessageRequest,
) -> Result<RemoteSendMessageResponse, FederationClientError> {
    if !request.locators_are_bounded() {
        return Err(FederationClientError::RemoteRejected(
            "send locators exceed the request budget".into(),
        ));
    }
    if request.destination_instance_id != peer.peer_instance_id {
        return Err(FederationClientError::DestinationMismatch);
    }
    let caller = local_db.instance_id().await?;
    let endpoint = peer
        .base_url
        .navigation_url("/api/federation/peer/send-conversation-message")?;
    let response = remote_query_client(&peer.tls_trust)?
        .post(endpoint.as_url().clone())
        .bearer_auth(peer.bearer_credential.expose())
        .json(request)
        .send()
        .await?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, MAX_REMOTE_RESPONSE_BYTES)?;
    }
    let mut response = decode_remote_send_response(status, &bytes, caller, request)?;
    if let RemoteSendReceipt::Accepted { acceptance }
    | RemoteSendReceipt::Duplicate { acceptance } = &mut response.receipt
    {
        for navigation in [
            &mut acceptance.stable_navigation,
            &mut acceptance.accepting_transcript_navigation,
            &mut acceptance.accepting_message_navigation,
        ] {
            let RemoteSendNavigation::AppLocal { path } = navigation else {
                return Err(FederationClientError::SendProvenanceMismatch);
            };
            let url = peer.base_url.navigation_url(path)?.as_url().to_string();
            *navigation = RemoteSendNavigation::Peer { url };
        }
    }
    Ok(response)
}

fn decode_remote_send_response(
    status: StatusCode,
    bytes: &[u8],
    caller: InstanceId,
    request: &RemoteSendMessageRequest,
) -> Result<RemoteSendMessageResponse, FederationClientError> {
    if !status.is_success() {
        return Err(FederationClientError::RemoteRejected(format!(
            "HTTP {status}"
        )));
    }
    let response: RemoteSendMessageResponse = serde_json::from_slice(bytes)?;
    if response.destination_instance_id != request.destination_instance_id {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != caller {
        return Err(FederationClientError::CallerMismatch);
    }
    if response.message_id != request.message_id || response.target != request.target {
        return Err(FederationClientError::SendProvenanceMismatch);
    }
    if let RemoteSendReceipt::Accepted { acceptance }
    | RemoteSendReceipt::Duplicate { acceptance } = &response.receipt
    {
        if acceptance.stable_destination.instance_id != request.destination_instance_id {
            return Err(FederationClientError::SendProvenanceMismatch);
        }
        let RemoteSendNavigation::AppLocal { path: stable_path } = &acceptance.stable_navigation
        else {
            return Err(FederationClientError::SendProvenanceMismatch);
        };
        let stable_shape = super::product_conversations::canonical_product_route_shape(stable_path);
        let stable_matches = match &request.target {
            RemoteSendTarget::ProductConversation { .. } => stable_shape,
            RemoteSendTarget::Coordinator {} => stable_path == "/global",
            RemoteSendTarget::Transcript { .. } => stable_shape || stable_path == "/global",
        };
        let exact_path =
            super::product_conversations::exact_transcript_route(&acceptance.accepting_transcript);
        let message_id = accepting_message_id(
            &acceptance.accepting_transcript,
            request.message_id,
            acceptance.disposition,
        );
        let message_path = format!(
            "{exact_path}{}",
            super::product_conversations::message_fragment(&message_id)
        );
        if !stable_matches
            || acceptance.accepting_transcript_navigation
                != (RemoteSendNavigation::AppLocal { path: exact_path })
            || acceptance.accepting_message_navigation
                != (RemoteSendNavigation::AppLocal { path: message_path })
        {
            return Err(FederationClientError::SendProvenanceMismatch);
        }
        match &request.target {
            RemoteSendTarget::ProductConversation {
                product_conversation_id,
            } if product_conversation_id
                != &acceptance.stable_destination.product_conversation_id =>
            {
                return Err(FederationClientError::SendProvenanceMismatch)
            }
            RemoteSendTarget::Transcript { transcript_id }
                if transcript_id != &acceptance.accepting_transcript =>
            {
                return Err(FederationClientError::SendProvenanceMismatch)
            }
            RemoteSendTarget::ProductConversation { .. }
            | RemoteSendTarget::Transcript { .. }
            | RemoteSendTarget::Coordinator {} => {}
        }
    }
    Ok(response)
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
pub struct RemoteResolveReferenceRequest {
    pub caller_instance_id: InstanceId,
    pub destination_instance_id: InstanceId,
    pub reference: String,
}

#[derive(Serialize, Deserialize)]
pub struct RemoteResolveReferenceResponse {
    pub caller_instance_id: InstanceId,
    pub destination_instance_id: InstanceId,
    pub resolution: crate::api::global_read::ResolveGlobalReferenceResponse,
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
enum RemoteResolveErrorCode {
    InvalidReference,
    DestinationUnavailable,
    ResponseTooLarge,
    CallerMismatch,
    DestinationMismatch,
}

#[derive(Serialize, Deserialize)]
struct RemoteResolveErrorResponse {
    code: RemoteResolveErrorCode,
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
    #[error("subscription enrollment authentication rejected")]
    SubscriptionAuthenticationRejected,
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
    #[error("remote send receipt provenance mismatch")]
    SendProvenanceMismatch,
    #[error("remote read response provenance mismatch")]
    ReadProvenanceMismatch,
    #[error("remote read target rejected: {0}")]
    ReadInvalidTarget(String),
    #[error("remote read destination unavailable: {0}")]
    ReadUnavailable(String),
    #[error("remote read response exceeds the bounded response budget: {0}")]
    ReadResponseTooLarge(String),
    #[error("remote reference rejected: {0}")]
    ResolveInvalidReference(String),
    #[error("remote reference destination unavailable: {0}")]
    ResolveUnavailable(String),
    #[error("remote reference response exceeds the bounded response budget: {0}")]
    ResolveResponseTooLarge(String),
    #[error("remote reference provenance mismatch")]
    ResolveProvenanceMismatch,
}

pub async fn resolve_remote_reference(
    local_db: &crate::db::Database,
    peer: &phoenix_db::FederationPeerConnection,
    reference: String,
) -> Result<crate::api::global_read::ResolveGlobalReferenceResponse, FederationClientError> {
    let caller_instance_id = local_db.instance_id().await?;
    let expected_target =
        crate::api::global_read::GlobalReadService::parse_read_target(&reference).ok();
    let endpoint = peer
        .base_url
        .navigation_url("/api/federation/peer/resolve-reference")?;
    let client = remote_query_client(&peer.tls_trust)?;
    let response = client
        .post(endpoint.as_url().clone())
        .bearer_auth(peer.bearer_credential.expose())
        .json(&RemoteResolveReferenceRequest {
            caller_instance_id,
            destination_instance_id: peer.peer_instance_id,
            reference,
        })
        .send()
        .await?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, MAX_REMOTE_RESPONSE_BYTES)?;
    }
    let mut response = decode_remote_resolve_response(status, &bytes)?;
    if response.destination_instance_id != peer.peer_instance_id {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != caller_instance_id {
        return Err(FederationClientError::CallerMismatch);
    }
    if response.resolution.reference.instance_id != peer.peer_instance_id {
        return Err(FederationClientError::ResolveProvenanceMismatch);
    }
    if expected_target
        .as_ref()
        .is_some_and(|target| target != &response.resolution.reference.target)
    {
        return Err(FederationClientError::ResolveProvenanceMismatch);
    }
    let path_owner_matches = match (
        &response.resolution.work_scope,
        response.resolution.path_owner_instance_id,
    ) {
        (crate::api::global_read::ResolvedWorkScope::Available { .. }, Some(instance_id)) => {
            instance_id == peer.peer_instance_id
        }
        (crate::api::global_read::ResolvedWorkScope::Available { .. }, None) | (_, Some(_)) => {
            false
        }
        (_, None) => true,
    };
    if !path_owner_matches {
        return Err(FederationClientError::ResolveProvenanceMismatch);
    }
    if let Some(path) = &response.resolution.href {
        if expected_target
            .as_ref()
            .is_some_and(|target| !resolved_reference_href_matches(target, path))
        {
            return Err(FederationClientError::ResolveProvenanceMismatch);
        }
        response.resolution.href = Some(peer.base_url.navigation_url(path)?.as_url().to_string());
    }
    Ok(response.resolution)
}

fn resolved_reference_href_matches(target: &ConversationReferenceTarget, href: &str) -> bool {
    use crate::api::product_conversations::{
        canonical_product_route_shape, exact_transcript_route, message_fragment,
    };

    match target {
        ConversationReferenceTarget::ProductConversation { .. } => {
            canonical_product_route_shape(href)
        }
        ConversationReferenceTarget::Transcript { transcript_id } => {
            href == exact_transcript_route(transcript_id)
        }
        ConversationReferenceTarget::Message {
            transcript_id,
            message_id,
        } => {
            let base = exact_transcript_route(transcript_id);
            href == base || href == format!("{base}{}", message_fragment(message_id.as_str()))
        }
        ConversationReferenceTarget::Coordinator => href == "/global",
    }
}

fn decode_remote_resolve_response(
    status: StatusCode,
    bytes: &[u8],
) -> Result<RemoteResolveReferenceResponse, FederationClientError> {
    if status.is_success() {
        return Ok(serde_json::from_slice(bytes)?);
    }
    let Some(error) = serde_json::from_slice::<RemoteResolveErrorResponse>(bytes).ok() else {
        return Err(FederationClientError::RemoteRejected(format!(
            "HTTP {status}"
        )));
    };
    Err(match error.code {
        RemoteResolveErrorCode::InvalidReference => {
            FederationClientError::ResolveInvalidReference(error.message)
        }
        RemoteResolveErrorCode::DestinationUnavailable => {
            FederationClientError::ResolveUnavailable(error.message)
        }
        RemoteResolveErrorCode::ResponseTooLarge => {
            FederationClientError::ResolveResponseTooLarge(error.message)
        }
        RemoteResolveErrorCode::CallerMismatch => FederationClientError::CallerMismatch,
        RemoteResolveErrorCode::DestinationMismatch => FederationClientError::DestinationMismatch,
    })
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
                Some(crate::api::global_read::QualifiedSearchNavigation::AppLocal { path })
                    if resolved_reference_href_matches(&hit.source.target, path)
            )
            || !matches!(
                &hit.source.target,
                ConversationReferenceTarget::Message { .. }
            )
            || match (&hit.stable_conversation, &hit.stable_navigation) {
                (None, None) => false,
                (
                    Some(stable),
                    Some(crate::api::global_read::QualifiedSearchNavigation::AppLocal { path }),
                ) => {
                    !super::product_conversations::canonical_product_route_shape(path)
                        || stable.instance_id != expected_destination
                        || !matches!(
                            &stable.target,
                            ConversationReferenceTarget::ProductConversation { .. }
                        )
                }
                _ => true,
            }
            || hit.sender.as_ref().is_some_and(|sender| {
                sender.stable.instance_id != sender.exact.instance_id
                    || !matches!(
                        &sender.stable.target,
                        ConversationReferenceTarget::ProductConversation { .. }
                            | ConversationReferenceTarget::Coordinator
                    )
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

pub async fn resolve_reference(
    authenticated: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteResolveReferenceRequest>,
) -> Response {
    let destination = match state.db.instance_id().await {
        Ok(instance_id) => instance_id,
        Err(error) => {
            tracing::error!(%error, "failed to read local instance identity");
            return remote_resolve_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                RemoteResolveErrorCode::DestinationUnavailable,
                "destination reference resolver unavailable",
            );
        }
    };
    if request.destination_instance_id != destination {
        return remote_resolve_error(
            StatusCode::CONFLICT,
            RemoteResolveErrorCode::DestinationMismatch,
            "destination instance identity mismatch",
        );
    }
    if request.caller_instance_id != authenticated.caller_instance_id {
        return remote_resolve_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            RemoteResolveErrorCode::CallerMismatch,
            "caller instance identity mismatch",
        );
    }
    let service = crate::api::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    match service.resolve_reference(&request.reference).await {
        Ok(resolution) => {
            let response = RemoteResolveReferenceResponse {
                caller_instance_id: authenticated.caller_instance_id,
                destination_instance_id: destination,
                resolution,
            };
            match serde_json::to_vec(&response) {
                Ok(encoded) if encoded.len() <= MAX_REMOTE_RESPONSE_BYTES => {
                    Json(response).into_response()
                }
                Ok(_) => remote_resolve_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    RemoteResolveErrorCode::ResponseTooLarge,
                    "remote reference response exceeds the bounded response budget",
                ),
                Err(error) => {
                    tracing::error!(%error, "failed to encode peer reference resolution");
                    remote_resolve_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        RemoteResolveErrorCode::DestinationUnavailable,
                        "destination reference resolver unavailable",
                    )
                }
            }
        }
        Err(
            crate::api::handlers::AppError::BadRequest(message)
            | crate::api::handlers::AppError::NotFound(message),
        ) => remote_resolve_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            RemoteResolveErrorCode::InvalidReference,
            &message,
        ),
        Err(error) => {
            tracing::error!(?error, "peer reference resolution failed");
            remote_resolve_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                RemoteResolveErrorCode::DestinationUnavailable,
                "destination reference resolver unavailable",
            )
        }
    }
}

fn remote_resolve_error(
    status: StatusCode,
    code: RemoteResolveErrorCode,
    message: &str,
) -> Response {
    (
        status,
        Json(RemoteResolveErrorResponse {
            code,
            message: message.to_string(),
        }),
    )
        .into_response()
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
mod send_tests;

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

    #[test]
    fn resolved_routes_distinguish_canonical_navigation_from_exact_evidence() {
        let product = ConversationReferenceTarget::ProductConversation {
            product_conversation_id:
                phoenix_core::domain::product_conversation::ProductConversationId::parse(
                    "stable-product-id",
                )
                .unwrap(),
        };
        assert!(resolved_reference_href_matches(
            &product,
            "/c/readable-name"
        ));
        assert!(resolved_reference_href_matches(
            &product,
            "/c/%C3%BCber-%E6%9D%B1%E4%BA%AC"
        ));
        for invalid in [
            "/c/",
            "/c/readable-name/extra",
            "/c/readable-name?source_transcript=other",
            "https://peer.example/c/readable-name",
        ] {
            assert!(!resolved_reference_href_matches(&product, invalid));
        }

        let transcript = ConversationReferenceTarget::Transcript {
            transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse(
                "exact-transcript",
            )
            .unwrap(),
        };
        assert!(resolved_reference_href_matches(
            &transcript,
            "/c/exact-transcript?source_transcript=exact-transcript"
        ));
        assert!(!resolved_reference_href_matches(
            &transcript,
            "/c/readable-name"
        ));
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

    async fn assert_remote_redirect_rejected(
        state: &AppState,
        peer: InstanceId,
        send_message: bool,
    ) {
        let result = if send_message {
            let connection = state
                .db
                .federation_peer_connection(peer)
                .await
                .unwrap()
                .unwrap();
            send_remote_conversation_message(
                &state.db,
                &connection,
                &RemoteSendMessageRequest {
                    destination_instance_id: peer,
                    target: RemoteSendTarget::Coordinator {},
                    message_id: uuid::Uuid::new_v4(),
                    message: RemoteMessageText::parse("redirect test".into()).unwrap(),
                    sender: RemoteSenderAttestation::RemoteApi {},
                },
            )
            .await
            .map(|_| ())
        } else {
            query_remote_database(&state.db, peer, "SELECT 1")
                .await
                .map(|_| ())
        };
        assert!(matches!(
            result,
            Err(FederationClientError::RemoteRejected(detail))
                if detail.starts_with("HTTP 30")
        ));
    }

    async fn save_redirect_peer(
        state: &AppState,
        peer: InstanceId,
        source_address: std::net::SocketAddr,
        token: &phoenix_core::domain::instance_identity::PeerBearerCredential,
        trust: &PeerTlsTrust,
    ) {
        state
            .db
            .save_federation_peer_connection(
                peer,
                "peer",
                &format!("https://localhost:{}", source_address.port())
                    .parse()
                    .unwrap(),
                token,
                trust,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn authenticated_remote_query_and_send_do_not_follow_307_or_308() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let peer = InstanceId::new();
        let token = phoenix_core::domain::instance_identity::PeerBearerCredential::parse(format!(
            "phx_peer_{}",
            "a".repeat(43)
        ))
        .unwrap();

        for (status, cross_origin, send_message) in [
            ("307 Temporary Redirect", false, false),
            ("308 Permanent Redirect", true, false),
            ("307 Temporary Redirect", false, true),
            ("308 Permanent Redirect", true, true),
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
                let operation = if send_message {
                    "send-conversation-message"
                } else {
                    "query-database"
                };
                assert!(request.starts_with(&format!("POST /api/federation/peer/{operation} ")));
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
            save_redirect_peer(&state, peer, source_address, &token, &trust).await;

            assert_remote_redirect_rejected(&state, peer, send_message).await;
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
                enrollment_id: uuid::Uuid::new_v4(),
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
