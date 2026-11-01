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
use phoenix_core::domain::{
    close::TranscriptConversationId,
    conversation_goal::{GoalHistoryEntry, GoalVersion},
    product_conversation::ProductConversationId,
};
use phoenix_db::conversation_goals::{
    CoordinatorGoalWaitRequest, CoordinatorGoalWaitStatus, GoalDbError,
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
const MAX_REMOTE_GOAL_HISTORY_ENTRIES: usize = 100;
const REMOTE_STABLE_CONTINUATION_ERROR: &str = "remote @conv continuation is not source-bound; read @conv at cursor 0, then continue with the exact @transcript:<id> shown in that page's evidence header";

fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

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
}

impl RemoteSendTarget {
    pub(crate) fn parse_tool_target(raw: &str) -> Result<Self, String> {
        if raw != raw.trim() || raw.contains('#') {
            return Err(
                "send targets must be @conv:<id> or @transcript:<id> without a message fragment"
                    .into(),
            );
        }
        match RemoteConversationTarget::parse(raw).map_err(|error| error.to_string())? {
            RemoteConversationTarget::ProductConversation {
                product_conversation_id,
            } => Ok(Self::ProductConversation {
                product_conversation_id:
                    phoenix_core::domain::product_conversation::ProductConversationId::parse(
                        product_conversation_id,
                    )
                    .map_err(|error| error.to_string())?,
            }),
            RemoteConversationTarget::ExactTranscript { transcript_id, .. } => {
                Ok(Self::Transcript {
                    transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse(
                        transcript_id,
                    )
                    .map_err(|error| error.to_string())?,
                })
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
    pub admitted_message_id: String,
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
    AuthorityInactive,
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
    pub operation_id: uuid::Uuid,
    pub receipt: RemoteSendReceipt,
}

#[derive(Default)]
pub(crate) struct RemoteMessageAdmissionGates {
    gates: tokio::sync::Mutex<
        std::collections::HashMap<uuid::Uuid, std::sync::Weak<tokio::sync::Mutex<()>>>,
    >,
}

impl RemoteMessageAdmissionGates {
    async fn lock(&self, message_id: uuid::Uuid) -> tokio::sync::OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self.gates.lock().await;
            if gates.len() >= 1024 {
                gates.retain(|_, gate| gate.strong_count() > 0);
            }
            if let Some(gate) = gates.get(&message_id).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = std::sync::Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(message_id, std::sync::Arc::downgrade(&gate));
                gate
            }
        };
        gate.lock_owned().await
    }
}

fn remote_admission_id(caller: InstanceId, operation_id: uuid::Uuid) -> uuid::Uuid {
    const NAMESPACE: uuid::Uuid = uuid::Uuid::from_u128(0xc3ae2bb7_152d_52f4_978a_21b027a4dc91);
    let caller = uuid::Uuid::parse_str(&caller.to_string()).expect("typed instance UUID");
    let mut name = [0_u8; 32];
    name[..16].copy_from_slice(caller.as_bytes());
    name[16..].copy_from_slice(operation_id.as_bytes());
    uuid::Uuid::new_v5(&NAMESPACE, &name)
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
    let caller = authenticated.caller_instance_id;
    let authority = authenticated.send_authority;
    let operation_id = request.message_id;
    let target = request.target.clone();
    let receipt = if request.destination_instance_id == destination {
        // Admission outlives an HTTP disconnect, including its per-operation gate.
        let admission = tokio::spawn(async move {
            let admitted_id = remote_admission_id(caller, operation_id);
            let _guard = state.remote_message_admission.lock(admitted_id).await;
            admit_remote_message(&state, caller, admitted_id, &request, &authority).await
        });
        match admission.await {
            Ok(receipt) => receipt,
            Err(error) => {
                tracing::error!(%error, "federation send admission task failed");
                RemoteSendReceipt::Rejected {
                    code: RemoteSendRejectionCode::Internal,
                    message: "remote message admission failed".into(),
                }
            }
        }
    } else {
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::DestinationMismatch,
            message: "destination instance identity mismatch".into(),
        }
    };
    Json(RemoteSendMessageResponse {
        destination_instance_id: destination,
        caller_instance_id: caller,
        target,
        operation_id,
        receipt,
    })
    .into_response()
}

async fn resolve_remote_send_target(
    state: &AppState,
    target: &RemoteSendTarget,
) -> Result<crate::send_chat_service::SendChatTarget, RemoteSendReceipt> {
    use crate::send_chat_service::SendChatTarget;
    let rejected = |code, message: String| RemoteSendReceipt::Rejected {
        code,
        message: bounded_send_diagnostic(&message),
    };
    let raw = match target {
        RemoteSendTarget::ProductConversation {
            product_conversation_id,
        } => format!("@conv:{product_conversation_id}"),
        RemoteSendTarget::Transcript { transcript_id } => format!("@transcript:{transcript_id}"),
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
    admitted_id: uuid::Uuid,
    request: &RemoteSendMessageRequest,
    authority: &phoenix_db::FederationSendAuthority,
) -> RemoteSendReceipt {
    use crate::send_chat_service::{
        MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatReplay,
        SendChatRequest, SendChatServiceError,
    };
    use sha2::Digest as _;
    let message_id = admitted_id.to_string();
    let owner = state.db.remote_send_message_owner(&message_id).await;
    let target = match owner {
        Ok(Some(owner)) => crate::send_chat_service::SendChatTarget::ExactTranscript(owner),
        Ok(None) => match resolve_remote_send_target(state, &request.target).await {
            Ok(target) => target,
            Err(receipt) => return receipt,
        },
        Err(error) => {
            return RemoteSendReceipt::Rejected {
                code: RemoteSendRejectionCode::Internal,
                message: bounded_send_diagnostic(&error.to_string()),
            };
        }
    };
    let target_fingerprint = sha2::Sha256::digest(
        serde_json::to_vec(&request.target).expect("typed remote target serializes"),
    )
    .into();
    let service = SendChatApplicationService::new(state.db.clone(), state.runtime.clone());
    let admission = service
        .send_federated_to_target_with_replay(
            target,
            SendChatRequest {
                conversation_id: String::new(),
                origin: request.sender.clone().into_origin(caller),
                text: request.message.0.clone(),
                message_id,
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                expansion_policy: MessageExpansionPolicy::RemoteLiteralText { target_fingerprint },
            },
            authority,
        )
        .await;
    let (id, outcome, replay) = match admission {
        Ok(result) => result,
        Err(error) => {
            let code = match &error {
                SendChatServiceError::IdempotencyConflict => {
                    RemoteSendRejectionCode::IdempotencyConflict
                }
                SendChatServiceError::NotFound(_) => RemoteSendRejectionCode::TargetNotFound,
                SendChatServiceError::FederationSendAuthorityInactive => {
                    RemoteSendRejectionCode::AuthorityInactive
                }
                SendChatServiceError::Busy => RemoteSendRejectionCode::Busy,
                SendChatServiceError::CloseAdmissionFenced
                | SendChatServiceError::HistoryUnavailable => {
                    RemoteSendRejectionCode::TargetUnavailable
                }
                SendChatServiceError::AttachmentValidation(_)
                | SendChatServiceError::Expansion { .. } => {
                    RemoteSendRejectionCode::AdmissionRejected
                }
                SendChatServiceError::Internal(_) | SendChatServiceError::Dispatch(_) => {
                    RemoteSendRejectionCode::Internal
                }
            };
            return RemoteSendReceipt::Rejected {
                code,
                message: bounded_send_diagnostic(&error.to_string()),
            };
        }
    };
    if let SendChatOutcome::Rejected { message, .. } = outcome {
        return RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::AdmissionRejected,
            message: bounded_send_diagnostic(&message),
        };
    }
    let disposition =
        if replay == SendChatReplay::Steering || outcome == SendChatOutcome::QueuedAsSteering {
            RemoteSendDisposition::Queued
        } else {
            RemoteSendDisposition::Delivered
        };
    let acceptance =
        match build_remote_message_acceptance(state, request, id, admitted_id, disposition).await {
            Ok(acceptance) => acceptance,
            Err(receipt) => return receipt,
        };
    if replay == SendChatReplay::New {
        RemoteSendReceipt::Accepted { acceptance }
    } else {
        RemoteSendReceipt::Duplicate { acceptance }
    }
}

async fn build_remote_message_acceptance(
    state: &AppState,
    request: &RemoteSendMessageRequest,
    id: String,
    admitted_id: uuid::Uuid,
    disposition: RemoteSendDisposition,
) -> Result<RemoteMessageAcceptance, RemoteSendReceipt> {
    let conversation =
        state
            .db
            .get_conversation(&id)
            .await
            .map_err(|error| RemoteSendReceipt::Rejected {
                code: RemoteSendRejectionCode::Internal,
                message: bounded_send_diagnostic(&error.to_string()),
            })?;
    let stable_path = if conversation.runtime_role == crate::work_scope::RuntimeRole::Coordinator {
        "/global".to_string()
    } else {
        super::product_conversations::product_route(
            None,
            conversation.product_conversation_id.as_str(),
        )
    };
    let transcript = phoenix_core::domain::close::TranscriptConversationId::parse(id)
        .expect("persisted transcript is nonempty");
    let exact_path = super::global_read::exact_conversation_href(transcript.as_str(), None);
    let message_id = accepting_message_id(&transcript, admitted_id, disposition);
    Ok(RemoteMessageAcceptance {
        admitted_message_id: message_id.clone(),
        accepting_transcript: transcript,
        stable_navigation: RemoteSendNavigation::AppLocal { path: stable_path },
        accepting_transcript_navigation: RemoteSendNavigation::AppLocal {
            path: exact_path.clone(),
        },
        accepting_message_navigation: RemoteSendNavigation::AppLocal {
            path: format!(
                "{exact_path}#message-{}",
                super::product_conversations::percent_encode_route_segment(&message_id)
            ),
        },
        stable_destination: RemoteStableDestination {
            instance_id: request.destination_instance_id,
            product_conversation_id: conversation.product_conversation_id,
        },
        disposition,
    })
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
    let endpoint = format!(
        "{}api/federation/peer/send-conversation-message",
        peer.base_url
    );
    let response = remote_query_client(&peer.tls_trust)?
        .post(endpoint)
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
            let url = format!("{}{}", peer.base_url, path.trim_start_matches('/'));
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
    if response.operation_id != request.message_id || response.target != request.target {
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
        let stable_shape = stable_path == "/global"
            || stable_path
                == &format!(
                    "/c/{}",
                    super::product_conversations::percent_encode_route_segment(
                        acceptance
                            .stable_destination
                            .product_conversation_id
                            .as_str()
                    )
                );
        let stable_matches = stable_shape;
        let exact_path = super::global_read::exact_conversation_href(
            acceptance.accepting_transcript.as_str(),
            None,
        );
        let message_id = accepting_message_id(
            &acceptance.accepting_transcript,
            remote_admission_id(caller, request.message_id),
            acceptance.disposition,
        );
        let message_path = format!(
            "{exact_path}#message-{}",
            super::product_conversations::percent_encode_route_segment(&message_id)
        );
        if !stable_matches
            || acceptance.admitted_message_id != message_id
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
            RemoteSendTarget::ProductConversation { .. } | RemoteSendTarget::Transcript { .. } => {}
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

#[derive(Debug, thiserror::Error)]
pub enum FederationClientError {
    #[error("remote send response provenance mismatch")]
    SendProvenanceMismatch,
    #[error("peer connection not found")]
    PeerNotFound,
    #[error("peer display name is ambiguous")]
    PeerAmbiguous,
    #[error("peer selector must not be empty")]
    InvalidPeerSelector,
    #[error("invalid Goal wait request: {0}")]
    InvalidGoalWait(String),
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

fn validate_remote_cursor_target(
    target: &RemoteConversationTarget,
    cursor: usize,
) -> Result<(), &'static str> {
    if cursor > 0 && matches!(target, RemoteConversationTarget::ProductConversation { .. }) {
        return Err(REMOTE_STABLE_CONTINUATION_ERROR);
    }
    Ok(())
}

impl RemoteReadConversationRequest {
    fn validate_cursor_target(&self) -> Result<(), &'static str> {
        validate_remote_cursor_target(&self.target, self.cursor)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteReadConversationResponse {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub source_instance_id: InstanceId,
    pub requested: RemoteConversationTarget,
    pub cursor: usize,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub next_cursor: Option<usize>,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteGoalWaitRequest {
    minimum_version: GoalVersion,
    timeout_seconds: u64,
}

impl RemoteGoalWaitRequest {
    fn from_coordinator_request(
        request: CoordinatorGoalWaitRequest,
    ) -> Result<Self, FederationClientError> {
        let timeout = request.timeout();
        if timeout.subsec_nanos() != 0 {
            return Err(FederationClientError::InvalidGoalWait(
                "timeout must use whole seconds".to_string(),
            ));
        }
        Ok(Self {
            minimum_version: request.minimum_version(),
            timeout_seconds: timeout.as_secs(),
        })
    }

    fn into_coordinator_request(
        self,
    ) -> Result<
        CoordinatorGoalWaitRequest,
        phoenix_db::conversation_goals::CoordinatorGoalWaitRequestError,
    > {
        CoordinatorGoalWaitRequest::new(
            self.minimum_version,
            std::time::Duration::from_secs(self.timeout_seconds),
        )
    }

    const fn minimum_version(self) -> GoalVersion {
        self.minimum_version
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteReadGoalRequest {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub target: ProductConversationId,
    pub after_version: GoalVersion,
    #[serde(default)]
    pub wait: Option<RemoteGoalWaitRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteGoalWaitStatus {
    Immediate,
    Satisfied,
    TimedOut,
}

impl From<CoordinatorGoalWaitStatus> for RemoteGoalWaitStatus {
    fn from(status: CoordinatorGoalWaitStatus) -> Self {
        match status {
            CoordinatorGoalWaitStatus::Satisfied => Self::Satisfied,
            CoordinatorGoalWaitStatus::TimedOut => Self::TimedOut,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteReadGoalResponse {
    pub source_instance_id: InstanceId,
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub target: ProductConversationId,
    pub exact_transcript_id: TranscriptConversationId,
    pub after_version: GoalVersion,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub wait: Option<RemoteGoalWaitRequest>,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub current: Option<GoalHistoryEntry>,
    pub history: Vec<GoalHistoryEntry>,
    pub wait_status: RemoteGoalWaitStatus,
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
        if db.federation_peer_connection(id).await?.is_some() {
            return Ok(id);
        }
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
    validate_remote_cursor_target(&requested, cursor)
        .map_err(|error| FederationClientError::RemoteRejected(error.to_string()))?;
    let peer_id = resolve_saved_peer_id(db, selector).await?;
    let peer = db
        .federation_peer_connection(peer_id)
        .await?
        .ok_or(FederationClientError::PeerNotFound)?;
    let caller = db.instance_id().await?;
    let request = RemoteReadConversationRequest {
        destination_instance_id: peer_id,
        target: requested.clone(),
        cursor,
    };
    let response: RemoteReadConversationResponse = send_bounded_peer_request(
        format!("{}api/federation/peer/read-conversation", peer.base_url),
        peer.bearer_credential.expose(),
        &peer.tls_trust,
        &request,
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

fn validate_remote_goal_response(
    response: &RemoteReadGoalResponse,
    peer_id: InstanceId,
    caller: InstanceId,
    request: &RemoteReadGoalRequest,
) -> Result<(), FederationClientError> {
    if response.source_instance_id != peer_id || response.destination_instance_id != peer_id {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != caller {
        return Err(FederationClientError::CallerMismatch);
    }
    if response.target != request.target {
        return Err(FederationClientError::ProvenanceMismatch(
            "goal response changed the stable target".to_string(),
        ));
    }
    if response.after_version != request.after_version {
        return Err(FederationClientError::ProvenanceMismatch(
            "goal response changed the history cursor".to_string(),
        ));
    }
    if response.wait != request.wait {
        return Err(FederationClientError::ProvenanceMismatch(
            "goal response changed the wait request".to_string(),
        ));
    }
    if response.history.len() > MAX_REMOTE_GOAL_HISTORY_ENTRIES {
        return Err(FederationClientError::ProvenanceMismatch(
            "goal response exceeded the history page bound".to_string(),
        ));
    }
    if response
        .current
        .iter()
        .chain(response.history.iter())
        .any(|entry| entry.snapshot().owner() != &request.target)
    {
        return Err(FederationClientError::ProvenanceMismatch(
            "goal response returned data for a different stable target".to_string(),
        ));
    }

    let current_version = response
        .current
        .as_ref()
        .map_or(GoalVersion::INITIAL, |entry| entry.snapshot().version());
    let history_distance = if current_version > request.after_version {
        current_version.get() - request.after_version.get()
    } else {
        0
    };
    let expected_history_len = usize::try_from(history_distance)
        .unwrap_or(MAX_REMOTE_GOAL_HISTORY_ENTRIES)
        .min(MAX_REMOTE_GOAL_HISTORY_ENTRIES);
    if response.history.len() != expected_history_len {
        return Err(FederationClientError::ProvenanceMismatch(
            "goal response returned an incomplete history page".to_string(),
        ));
    }

    let mut previous_version = request.after_version;
    let mut previous_entry = None;
    for entry in &response.history {
        let expected_version = previous_version.get().checked_add(1).ok_or_else(|| {
            FederationClientError::ProvenanceMismatch(
                "goal response history version overflowed".to_string(),
            )
        })?;
        if entry.snapshot().version().get() != expected_version {
            return Err(FederationClientError::ProvenanceMismatch(
                "goal response history was not contiguous after the requested cursor".to_string(),
            ));
        }
        if let Some(previous_entry) = previous_entry {
            match entry.mutation().advance(Some(previous_entry.snapshot())) {
                Ok(snapshot) if &snapshot == entry.snapshot() => {}
                _ => {
                    return Err(FederationClientError::ProvenanceMismatch(
                        "goal response history contained an impossible transition".to_string(),
                    ));
                }
            }
        } else if request.after_version == GoalVersion::INITIAL {
            match entry.mutation().advance(None) {
                Ok(snapshot) if &snapshot == entry.snapshot() => {}
                _ => {
                    return Err(FederationClientError::ProvenanceMismatch(
                        "goal response history did not begin with a valid Goal creation"
                            .to_string(),
                    ));
                }
            }
        }
        previous_version = entry.snapshot().version();
        previous_entry = Some(entry);
    }

    if let (Some(current), Some(last)) = (response.current.as_ref(), response.history.last()) {
        if current.snapshot().version() == last.snapshot().version() && current != last {
            return Err(FederationClientError::ProvenanceMismatch(
                "goal response current Goal differed from its history entry".to_string(),
            ));
        }
    }

    match (&request.wait, response.wait_status) {
        (None, RemoteGoalWaitStatus::Immediate) => Ok(()),
        (None, _) | (Some(_), RemoteGoalWaitStatus::Immediate) => {
            Err(FederationClientError::ProvenanceMismatch(
                "goal response returned an invalid wait status".to_string(),
            ))
        }
        (Some(wait), RemoteGoalWaitStatus::Satisfied) => {
            let version = response
                .current
                .as_ref()
                .map_or(GoalVersion::INITIAL, |entry| entry.snapshot().version());
            if version < wait.minimum_version() {
                return Err(FederationClientError::ProvenanceMismatch(
                    "goal response claimed satisfaction below the minimum version".to_string(),
                ));
            }
            Ok(())
        }
        (Some(wait), RemoteGoalWaitStatus::TimedOut) => {
            let version = response
                .current
                .as_ref()
                .map_or(GoalVersion::INITIAL, |entry| entry.snapshot().version());
            if version >= wait.minimum_version() {
                return Err(FederationClientError::ProvenanceMismatch(
                    "goal response claimed timeout at or above the minimum version".to_string(),
                ));
            }
            Ok(())
        }
    }
}

pub async fn read_remote_goal(
    db: &crate::db::Database,
    selector: &str,
    target: &ProductConversationId,
    after_version: GoalVersion,
    wait: Option<CoordinatorGoalWaitRequest>,
) -> Result<RemoteReadGoalResponse, FederationClientError> {
    let peer_id = resolve_saved_peer_id(db, selector).await?;
    let peer = db
        .federation_peer_connection(peer_id)
        .await?
        .ok_or(FederationClientError::PeerNotFound)?;
    let caller = db.instance_id().await?;
    let wait = wait
        .map(RemoteGoalWaitRequest::from_coordinator_request)
        .transpose()?;
    let request = RemoteReadGoalRequest {
        destination_instance_id: peer_id,
        caller_instance_id: caller,
        target: target.clone(),
        after_version,
        wait,
    };
    let response: RemoteReadGoalResponse = send_bounded_peer_request(
        format!("{}api/federation/peer/read-goal", peer.base_url),
        peer.bearer_credential.expose(),
        &peer.tls_trust,
        &request,
    )
    .await?;
    validate_remote_goal_response(&response, peer_id, caller, &request)?;
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
    Box::pin(resolve_reference_with_limits(
        peer,
        state,
        request,
        &READ_ADMISSION,
        REMOTE_READ_DEADLINE,
    ))
    .await
}

async fn resolve_reference_with_limits(
    peer: PeerAuthenticated,
    state: AppState,
    request: RemoteResolveReferenceRequest,
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
    if let Err(error) = require_canonical_target(&request.target) {
        return federation_error(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string());
    }
    let service = super::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    let reference = request.target.reference();
    let resolution = match execute_bounded_remote_read(admission, deadline, async move {
        service
            .resolve_reference_bounded(
                &reference,
                std::num::NonZeroUsize::new(MAX_REMOTE_RESPONSE_BYTES).unwrap(),
            )
            .await
    })
    .await
    {
        Err(error) => return remote_read_limit_response(error),
        Ok(Ok(resolution)) => resolution,
        Ok(Err(error)) => return error.into_response(),
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
    Box::pin(read_conversation_with_limits(
        peer,
        state,
        request,
        &READ_ADMISSION,
        REMOTE_READ_DEADLINE,
    ))
    .await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteReadLimitError {
    Admission,
    Deadline,
    Task,
}

fn remote_read_limit_response(error: RemoteReadLimitError) -> Response {
    match error {
        RemoteReadLimitError::Admission => federation_error(
            StatusCode::TOO_MANY_REQUESTS,
            "remote read admission limit reached",
        ),
        RemoteReadLimitError::Deadline => {
            federation_error(StatusCode::GATEWAY_TIMEOUT, "remote read deadline exceeded")
        }
        RemoteReadLimitError::Task => {
            federation_error(StatusCode::INTERNAL_SERVER_ERROR, "remote read task failed")
        }
    }
}

async fn execute_bounded_remote_read<T, F>(
    admission: &std::sync::Arc<tokio::sync::Semaphore>,
    deadline: std::time::Duration,
    operation: F,
) -> Result<T, RemoteReadLimitError>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let permit = std::sync::Arc::clone(admission)
        .try_acquire_owned()
        .map_err(|_| RemoteReadLimitError::Admission)?;
    let task = spawn_with_admission_permit(permit, operation);
    match tokio::time::timeout(deadline, task).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            tracing::error!(%error, "remote read task failed");
            Err(RemoteReadLimitError::Task)
        }
        Err(_) => Err(RemoteReadLimitError::Deadline),
    }
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
    if let Err(error) = request.validate_cursor_target() {
        return federation_error(StatusCode::BAD_REQUEST, error);
    }
    if let Err(error) = require_bounded_read_target(&request.target) {
        return federation_error(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string());
    }
    let service = super::global_read::GlobalReadService::new(
        state.db.clone(),
        state.message_retriever.clone(),
    );
    let reference = request.target.reference();
    let cursor = request.cursor;
    let read = execute_bounded_remote_read(admission, deadline, async move {
        service
            .read_conversation_page_bounded(
                &reference,
                cursor,
                std::num::NonZeroUsize::new(MAX_REMOTE_RESPONSE_BYTES).unwrap(),
            )
            .await
    })
    .await;
    match read {
        Err(error) => remote_read_limit_response(error),
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
        Ok(Err(error)) => bounded_read_error_response(error),
    }
}

pub async fn read_goal(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteReadGoalRequest>,
) -> Response {
    Box::pin(read_goal_with_limits(
        peer,
        state,
        request,
        &READ_ADMISSION,
        REMOTE_READ_DEADLINE,
    ))
    .await
}

async fn read_goal_with_limits(
    peer: PeerAuthenticated,
    state: AppState,
    request: RemoteReadGoalRequest,
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
    if request.caller_instance_id != peer.caller_instance_id {
        return federation_error(StatusCode::UNAUTHORIZED, "caller instance mismatch");
    }

    let db = state.db.clone();
    let target = request.target.clone();
    let after_version = request.after_version;
    let wait = request.wait;
    let effective_wait = match wait.map(RemoteGoalWaitRequest::into_coordinator_request) {
        Some(Ok(wait)) => wait,
        Some(Err(error)) => {
            return federation_error(StatusCode::BAD_REQUEST, &error.to_string());
        }
        None => {
            CoordinatorGoalWaitRequest::new(GoalVersion::INITIAL, std::time::Duration::from_secs(1))
                .expect("one second is a valid coordinator goal wait timeout")
        }
    };
    let authority = peer.send_authority.clone();
    let read = execute_bounded_remote_read(admission, deadline, async move {
        db.wait_for_federated_goals(&authority, &target, after_version, effective_wait)
            .await
    })
    .await;

    match read {
        Err(error) => remote_read_limit_response(error),
        Ok(Ok(result)) => {
            let wait_status = wait.map_or(RemoteGoalWaitStatus::Immediate, |_| {
                RemoteGoalWaitStatus::from(result.status())
            });
            Json(RemoteReadGoalResponse {
                source_instance_id: destination,
                destination_instance_id: destination,
                caller_instance_id: peer.caller_instance_id,
                target: request.target,
                exact_transcript_id: result.destination_transcript_id().clone(),
                after_version: request.after_version,
                wait,
                current: result.current().cloned(),
                history: result.history().to_vec(),
                wait_status,
            })
            .into_response()
        }
        Ok(Err(error)) => goal_read_error_response(error),
    }
}

fn goal_read_error_response(error: GoalDbError) -> Response {
    match error {
        GoalDbError::Invalid(message) => federation_error(StatusCode::BAD_REQUEST, &message),
        GoalDbError::Admission(phoenix_db::DbError::FederationSendAuthorityInactive) => {
            federation_error(
                StatusCode::FORBIDDEN,
                "Federation send authority is inactive.",
            )
        }
        GoalDbError::Admission(error) => {
            super::product_conversations::db_to_app(error).into_response()
        }
        error @ (GoalDbError::RetryConflict
        | GoalDbError::TargetChanged
        | GoalDbError::Busy
        | GoalDbError::BindingConflict) => {
            federation_error(StatusCode::CONFLICT, &error.to_string())
        }
        error @ GoalDbError::Domain(_) => {
            federation_error(StatusCode::BAD_REQUEST, &error.to_string())
        }
        error @ (GoalDbError::AutoContinueDisabled
        | GoalDbError::Control(_)
        | GoalDbError::DuplicateGoalId
        | GoalDbError::Sqlx(_)
        | GoalDbError::Integrity(_)) => {
            tracing::error!(%error, "remote goal read failed");
            federation_error(StatusCode::INTERNAL_SERVER_ERROR, "remote goal read failed")
        }
    }
}

fn bounded_read_error_response(error: super::global_read::BoundedReadError) -> Response {
    match error {
        super::global_read::BoundedReadError::ScanBudgetExhausted { limit } => federation_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "conversation scan budget of {limit} physical messages was exhausted; no page or EOF result was returned. This API version cannot resume the scan, and retrying the unchanged request will not advance; inspect the transcript locally."
            ),
        ),
        super::global_read::BoundedReadError::Read(message) => {
            federation_error(StatusCode::UNPROCESSABLE_ENTITY, &message)
        }
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

    mod goal_read;
    mod send;

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

    #[test]
    fn remote_read_response_requires_explicit_nullable_next_cursor() {
        let destination = InstanceId::new();
        let caller = InstanceId::new();
        let response = RemoteReadConversationResponse {
            destination_instance_id: destination,
            caller_instance_id: caller,
            source_instance_id: destination,
            requested: RemoteConversationTarget::ExactTranscript {
                transcript_id: "transcript".to_string(),
                message_id: None,
            },
            cursor: 0,
            next_cursor: None,
            content: "page".to_string(),
        };
        let explicit_null = serde_json::to_value(response).unwrap();
        let decoded: RemoteReadConversationResponse =
            serde_json::from_value(explicit_null.clone()).unwrap();
        assert_eq!(decoded.next_cursor, None);

        let mut explicit_number = explicit_null.clone();
        explicit_number["next_cursor"] = serde_json::json!(42);
        let decoded: RemoteReadConversationResponse =
            serde_json::from_value(explicit_number).unwrap();
        assert_eq!(decoded.next_cursor, Some(42));

        let mut missing = explicit_null;
        missing.as_object_mut().unwrap().remove("next_cursor");
        let error = serde_json::from_value::<RemoteReadConversationResponse>(missing).unwrap_err();
        assert!(error.to_string().contains("missing field `next_cursor`"));
    }

    #[test]
    fn remote_stable_reference_continuation_requires_exact_transcript() {
        let product = RemoteReadConversationRequest {
            destination_instance_id: InstanceId::new(),
            target: RemoteConversationTarget::ProductConversation {
                product_conversation_id: "product".to_string(),
            },
            cursor: 0,
        };
        assert_eq!(product.validate_cursor_target(), Ok(()));

        let product_continuation = RemoteReadConversationRequest {
            cursor: 1,
            ..product
        };
        assert_eq!(
            product_continuation.validate_cursor_target(),
            Err(REMOTE_STABLE_CONTINUATION_ERROR)
        );

        let exact_continuation = RemoteReadConversationRequest {
            destination_instance_id: InstanceId::new(),
            target: RemoteConversationTarget::ExactTranscript {
                transcript_id: "transcript".to_string(),
                message_id: None,
            },
            cursor: 1,
        };
        assert_eq!(exact_continuation.validate_cursor_target(), Ok(()));
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
        let uuid_shaped_name = InstanceId::new().to_string();
        let uuid_named_peer = InstanceId::new();
        state
            .db
            .save_federation_peer_connection(
                uuid_named_peer,
                &uuid_shaped_name,
                &"https://uuid-name.example".parse().unwrap(),
                &token("e"),
                &PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();

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
        assert_eq!(
            resolve_saved_peer_id(&state.db, &uuid_shaped_name)
                .await
                .unwrap(),
            uuid_named_peer
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

    #[tokio::test]
    async fn remote_read_cursor_target_guard_runs_before_peer_lookup() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;

        let continuation_error =
            read_remote_conversation(&state.db, "missing-peer", "@conv:missing-product", 1)
                .await
                .unwrap_err();
        assert!(matches!(
            continuation_error,
            FederationClientError::RemoteRejected(message)
                if message == REMOTE_STABLE_CONTINUATION_ERROR
        ));

        let first_page_error =
            read_remote_conversation(&state.db, "missing-peer", "@conv:missing-product", 0)
                .await
                .unwrap_err();
        assert!(matches!(
            first_page_error,
            FederationClientError::PeerNotFound
        ));

        let exact_error = read_remote_conversation(
            &state.db,
            "missing-peer",
            "@transcript:missing-transcript",
            1,
        )
        .await
        .unwrap_err();
        assert!(matches!(exact_error, FederationClientError::PeerNotFound));
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
        let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tracked_polls = polls.clone();
        let work = std::future::poll_fn(move |_| {
            tracked_polls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            std::task::Poll::Ready(1)
        });
        assert_eq!(
            execute_bounded_remote_read(&exhausted, std::time::Duration::from_secs(1), work).await,
            Err(RemoteReadLimitError::Admission)
        );
        assert_eq!(polls.load(std::sync::atomic::Ordering::Relaxed), 0);

        let admitted = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let deadline_admission = admitted.clone();
        let deadline_task = tokio::spawn(async move {
            execute_bounded_remote_read(
                &deadline_admission,
                std::time::Duration::from_millis(10),
                async move {
                    started_tx.send(()).unwrap();
                    finish_rx.await.unwrap();
                },
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), deadline_task)
                .await
                .unwrap()
                .unwrap(),
            Err(RemoteReadLimitError::Deadline)
        );
        assert!(admitted.clone().try_acquire_owned().is_err());
        finish_tx.send(()).unwrap();
        let released = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            admitted.clone().acquire_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        drop(released);

        let deadline_response = remote_read_limit_response(RemoteReadLimitError::Deadline);
        assert_eq!(deadline_response.status(), StatusCode::GATEWAY_TIMEOUT);
        let deadline_body =
            axum::body::to_bytes(deadline_response.into_body(), MAX_REMOTE_RESPONSE_BYTES)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&deadline_body).unwrap(),
            serde_json::json!({"error": "remote read deadline exceeded"})
        );
    }

    #[tokio::test]
    async fn resolve_reference_with_limits_enforces_admission_before_resolution() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let destination_instance_id = state.db.instance_id().await.unwrap();
        let peer = || {
            let caller = InstanceId::new();
            PeerAuthenticated {
            send_authority: phoenix_db::FederationSendAuthority::new(caller, phoenix_core::domain::instance_identity::FederationCredentialVerifier::from_bearer(b"peer-test")),
            caller_instance_id: caller,
            caller_display_name: "peer".to_string(),
        }
        };
        let request = || RemoteResolveReferenceRequest {
            destination_instance_id,
            target: RemoteConversationTarget::ExactTranscript {
                transcript_id: "missing-transcript".to_string(),
                message_id: Some("missing-message".to_string()),
            },
        };

        let exhausted = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let admission_response = resolve_reference_with_limits(
            peer(),
            state,
            request(),
            &exhausted,
            std::time::Duration::from_secs(1),
        )
        .await;
        assert_eq!(admission_response.status(), StatusCode::TOO_MANY_REQUESTS);
        let admission_body =
            axum::body::to_bytes(admission_response.into_body(), MAX_REMOTE_RESPONSE_BYTES)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&admission_body).unwrap(),
            serde_json::json!({"error": "remote read admission limit reached"})
        );
    }

    fn peer_read_conversation_request(
        bearer: &str,
        destination_instance_id: InstanceId,
        target: RemoteConversationTarget,
        cursor: usize,
    ) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/federation/peer/read-conversation")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {bearer}"))
            .body(axum::body::Body::from(
                serde_json::to_string(&RemoteReadConversationRequest {
                    destination_instance_id,
                    target,
                    cursor,
                })
                .unwrap(),
            ))
            .unwrap()
    }

    async fn decode_successful_remote_read(
        response: axum::response::Response,
    ) -> RemoteReadConversationResponse {
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), MAX_REMOTE_RESPONSE_BYTES)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn peer_read_conversation(
        app: &axum::Router,
        bearer: &str,
        destination_instance_id: InstanceId,
        target: RemoteConversationTarget,
        cursor: usize,
    ) -> axum::response::Response {
        use tower::ServiceExt as _;

        app.clone()
            .oneshot(peer_read_conversation_request(
                bearer,
                destination_instance_id,
                target,
                cursor,
            ))
            .await
            .unwrap()
    }

    async fn create_paginated_transition_source(state: &AppState) -> String {
        state
            .db
            .create_conversation("transition-t1", "transition", "/tmp", true, None, None)
            .await
            .unwrap();
        for index in 0..180 {
            state
                .db
                .add_message(
                    &format!("transition-t1-message-{index}"),
                    "transition-t1",
                    &crate::db::MessageContent::user(format!(
                        "transition T1 message {index} {}",
                        "x".repeat(80)
                    )),
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        sqlx::query_scalar(
            "SELECT product_conversation_id FROM conversations WHERE id = 'transition-t1'",
        )
        .fetch_one(state.db.pool())
        .await
        .unwrap()
    }

    async fn continue_transition_source(state: &AppState) -> String {
        use crate::db::ContinueOutcome;
        use phoenix_core::domain::sm_state::ConvState;

        state
            .db
            .update_conversation_state(
                "transition-t1",
                &ConvState::ContextExhausted {
                    summary: "continue to T2".to_string(),
                },
            )
            .await
            .unwrap();
        let ContinueOutcome::Created(successor) = state
            .db
            .continue_conversation("transition-t1")
            .await
            .unwrap()
        else {
            panic!("expected transition T2");
        };
        state
            .db
            .update_conversation_state(
                "transition-t1",
                &ConvState::HandedOff {
                    successor_conv_id: successor.id.clone(),
                },
            )
            .await
            .unwrap();
        state
            .db
            .add_message(
                "transition-t2-message",
                &successor.id,
                &crate::db::MessageContent::user("successor-only-marker".to_string()),
                None,
                None,
            )
            .await
            .unwrap();
        successor.id
    }

    #[tokio::test]
    async fn remote_pages_require_exact_transcript_after_product_continuation() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let product_id = create_paginated_transition_source(&state).await;
        let caller = InstanceId::new();
        let bearer = format!("phx_peer_{}", "c".repeat(43));
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
        let stable_target = || RemoteConversationTarget::ProductConversation {
            product_conversation_id: product_id.clone(),
        };
        let exact_t1 = || RemoteConversationTarget::ExactTranscript {
            transcript_id: "transition-t1".to_string(),
            message_id: None,
        };

        let first = peer_read_conversation(&app, &bearer, destination, stable_target(), 0).await;
        let first = decode_successful_remote_read(first).await;
        let next_cursor = first.next_cursor.expect("T1 must span multiple pages");
        assert!(first.content.contains("@transcript:transition-t1"));

        let successor_id = continue_transition_source(&state).await;
        let current = peer_read_conversation(&app, &bearer, destination, stable_target(), 0).await;
        let current = decode_successful_remote_read(current).await;
        assert!(current
            .content
            .contains(&format!("@transcript:{successor_id}")));
        assert!(current.content.contains("successor-only-marker"));

        let refused =
            peer_read_conversation(&app, &bearer, destination, stable_target(), next_cursor).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(refused.into_body(), MAX_REMOTE_RESPONSE_BYTES)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"error": REMOTE_STABLE_CONTINUATION_ERROR})
        );

        let pinned =
            peer_read_conversation(&app, &bearer, destination, exact_t1(), next_cursor).await;
        let pinned = decode_successful_remote_read(pinned).await;
        assert_eq!(pinned.cursor, next_cursor);
        assert_eq!(pinned.requested, exact_t1());
        assert!(pinned.content.contains("transition T1 message"));
        assert!(!pinned.content.contains("successor-only-marker"));
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
            .create_conversation(opaque_transcript_id, "opaque", "/tmp", true, None, None)
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
        let oversized_message_id = "remote-oversized-attachment-message";
        let hidden_image_payload = "base64-payload-must-not-cross-federation";
        let hidden_file_path = "/private/path/must-not-cross-federation.txt";
        state
            .db
            .add_message(
                oversized_message_id,
                "remote-transcript",
                &crate::db::MessageContent::user_with_attachments(
                    "z".repeat(MAX_REMOTE_RESPONSE_BYTES + 1),
                    vec![phoenix_core::domain::db_schema::ImageData {
                        data: hidden_image_payload.to_string(),
                        media_type: "image/png".to_string(),
                    }],
                    vec![phoenix_core::domain::db_schema::FileAttachment {
                        original_name: "private.txt".to_string(),
                        media_type: "text/plain".to_string(),
                        size_bytes: 42,
                        stored_path: hidden_file_path.to_string(),
                    }],
                ),
                None,
                None,
            )
            .await
            .unwrap();
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

        let oversized_exact = serde_json::to_value(RemoteResolveReferenceRequest {
            destination_instance_id: destination,
            target: RemoteConversationTarget::ExactTranscript {
                transcript_id: "remote-transcript".to_string(),
                message_id: Some(oversized_message_id.to_string()),
            },
        })
        .unwrap();
        let oversized_exact = app
            .clone()
            .oneshot(request(
                "/api/federation/peer/resolve-reference",
                oversized_exact,
                Some(&bearer),
            ))
            .await
            .unwrap();
        assert_eq!(oversized_exact.status(), StatusCode::OK);
        let body = axum::body::to_bytes(oversized_exact.into_body(), MAX_REMOTE_RESPONSE_BYTES)
            .await
            .unwrap();
        let oversized_exact: RemoteResolveReferenceResponse =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(
            oversized_exact.resolution.kind,
            super::super::global_read::ResolvedReferenceKind::Message
        );
        assert_eq!(oversized_exact.resolution.id, oversized_message_id);
        assert!(oversized_exact
            .resolution
            .summary
            .contains(super::super::global_read::OVERSIZED_REMOTE_MESSAGE_MARKER));
        assert!(oversized_exact
            .resolution
            .summary
            .contains("[2 attachment(s) are not shown in remote bounded reads]"));
        assert!(!oversized_exact
            .resolution
            .summary
            .contains(hidden_image_payload));
        assert!(!oversized_exact
            .resolution
            .summary
            .contains(hidden_file_path));

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
        assert!(serde_json::from_value::<RemoteResolveReferenceResponse>(valid.clone()).is_ok());

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

    #[tokio::test]
    async fn scan_budget_exhaustion_uses_actionable_422_federation_envelope() {
        let response = bounded_read_error_response(
            super::super::global_read::BoundedReadError::ScanBudgetExhausted { limit: 3 },
        );
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let message = value["error"].as_str().unwrap();
        assert!(message.contains("scan budget of 3 physical messages was exhausted"));
        assert!(message.contains("no page or EOF result was returned"));
        assert!(message.contains("retrying the unchanged request will not advance"));
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
    async fn cancelled_remote_read_caller_does_not_release_admission_before_work_finishes() {
        let admission = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let caller_admission = admission.clone();
        let caller = tokio::spawn(async move {
            execute_bounded_remote_read(
                &caller_admission,
                std::time::Duration::from_secs(30),
                async move {
                    started_tx.send(()).unwrap();
                    finish_rx.await.unwrap();
                },
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();

        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(admission.clone().try_acquire_owned().is_err());

        finish_tx.send(()).unwrap();
        let released = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            admission.clone().acquire_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        drop(released);
    }

    #[tokio::test]
    async fn query_database_rejects_exhausted_admission_before_sql_execution() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let destination_instance_id = state.db.instance_id().await.unwrap();
        let admission = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let caller = InstanceId::new();
        let response = query_database_with_admission(
            PeerAuthenticated {
                send_authority: phoenix_db::FederationSendAuthority::new(caller, phoenix_core::domain::instance_identity::FederationCredentialVerifier::from_bearer(b"peer-test")),
                caller_instance_id: caller,
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
