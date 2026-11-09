pub(crate) mod subscriptions;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use futures::StreamExt as _;
use phoenix_core::domain::instance_identity::{
    FederationCredentialVerifier, InstanceId, PeerDisplayName, PeerTlsTrust,
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
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::db::Database;
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
    pub caller_display_name: PeerDisplayName,
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
    pub peer_display_name: PeerDisplayName,
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

pub(crate) struct ResolvedPeer {
    connection: phoenix_db::FederationPeerConnection,
}

impl ResolvedPeer {
    pub(crate) fn instance_id(&self) -> InstanceId {
        self.connection.peer_instance_id
    }

    pub(crate) fn connection(&self) -> &phoenix_db::FederationPeerConnection {
        &self.connection
    }
}

async fn resolve_saved_peer(
    db: &crate::db::Database,
    selector: &str,
) -> Result<ResolvedPeer, FederationClientError> {
    let instance_id = resolve_saved_peer_id(db, selector).await?;
    let connection = db
        .federation_peer_connection(instance_id)
        .await?
        .ok_or(FederationClientError::PeerNotFound)?;
    Ok(ResolvedPeer { connection })
}

pub(crate) async fn resolve_tool_peer(
    db: &crate::db::Database,
    selector: &crate::coordinator_tools::instance::RemoteInstanceSelector,
    resolution: crate::coordinator_tools::instance::PeerResolution,
) -> Result<ResolvedPeer, FederationClientError> {
    if matches!(
        resolution,
        crate::coordinator_tools::instance::PeerResolution::WatchUuidIdentity
    ) {
        if let Ok(id) = InstanceId::from_str(selector.as_str()) {
            if selector.as_str() == id.to_string() {
                let connection = db
                    .federation_peer_connection(id)
                    .await?
                    .ok_or(FederationClientError::PeerNotFound)?;
                return Ok(ResolvedPeer { connection });
            }
        }
    }
    resolve_saved_peer(db, selector.as_str()).await
}

#[cfg(test)]
pub(crate) async fn resolve_saved_peer_for_test(
    db: &crate::db::Database,
    selector: &str,
) -> Result<ResolvedPeer, FederationClientError> {
    resolve_saved_peer(db, selector).await
}

pub async fn send_remote_conversation_message(
    local_db: &crate::db::Database,
    peer: &ResolvedPeer,
    request: &RemoteSendMessageRequest,
) -> Result<RemoteSendMessageResponse, FederationClientError> {
    if !request.locators_are_bounded() {
        return Err(FederationClientError::RemoteRejected(
            "send locators exceed the request budget".into(),
        ));
    }
    if request.destination_instance_id != peer.instance_id() {
        return Err(FederationClientError::DestinationMismatch);
    }
    let caller = local_db.instance_id().await?;
    let endpoint = format!(
        "{}api/federation/peer/send-conversation-message",
        peer.connection.base_url
    );
    let response = remote_query_client(&peer.connection.tls_trust)?
        .post(endpoint)
        .bearer_auth(peer.connection.bearer_credential.expose())
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
            let url = format!(
                "{}{}",
                peer.connection.base_url,
                path.trim_start_matches('/')
            );
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
                return Err(FederationClientError::SendProvenanceMismatch);
            }
            RemoteSendTarget::Transcript { transcript_id }
                if transcript_id != &acceptance.accepting_transcript =>
            {
                return Err(FederationClientError::SendProvenanceMismatch);
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
    #[error("peer selector resolves to this local instance")]
    SelfPeerSelector,
    #[error("subscription enrollment authentication rejected")]
    SubscriptionAuthenticationRejected,
    #[error("peer navigation URL is invalid: {0}")]
    Navigation(#[from] phoenix_core::domain::instance_identity::PeerNavigationUrlError),
    #[error("invalid Goal wait request: {0}")]
    InvalidGoalWait(String),
    #[error("peer database lookup failed: {0}")]
    Database(#[from] phoenix_db::DbError),
    #[error("peer selection failed: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("federation request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("native federation request exceeded {0} bytes")]
    NativeRequestTooLarge(usize),
    #[error("native federation endpoint is unsupported")]
    NativeEndpointUnsupported,
    #[error("native federation authentication, authorization, or identity was rejected: {0}")]
    NativeIdentityRejected(String),
    #[error("native federation protocol was rejected: {0}")]
    NativeProtocolRejected(String),
    #[error("native federation operation was rejected: {0}")]
    NativeDomainRejected(String),
    #[error("native federation destination failed: {0}")]
    NativeRemoteFailure(String),
    #[error("remote response exceeded {0} bytes")]
    ResponseTooLarge(usize),
    #[error(
        "create outcome uncertain at {destination_instance_id} for request {request_id}: {message}"
    )]
    NativeCreateUncertain {
        destination_instance_id: InstanceId,
        request_id: String,
        message: String,
    },
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
    peer: &ResolvedPeer,
    sql: &str,
) -> Result<RemoteQueryDatabaseResponse, FederationClientError> {
    let peer_instance_id = peer.instance_id();
    let peer = &peer.connection;
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
        if reference.trim() != reference
            || reference.len() > 1024
            || reference.chars().any(char::is_whitespace)
        {
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

    fn lookup_identity(&self) -> &str {
        match self {
            Self::ProductConversation {
                product_conversation_id,
            } => product_conversation_id,
            Self::ExactTranscript { transcript_id, .. } => transcript_id,
        }
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteAutomaticContinuationTarget {
    Ordinary {
        product_conversation_id: ProductConversationId,
    },
    Coordinator {},
}

impl RemoteAutomaticContinuationTarget {
    fn db_target(&self) -> phoenix_db::AutomaticContinuationTarget<'_> {
        match self {
            Self::Ordinary {
                product_conversation_id,
            } => phoenix_db::AutomaticContinuationTarget::Ordinary(product_conversation_id),
            Self::Coordinator {} => phoenix_db::AutomaticContinuationTarget::Coordinator,
        }
    }

    fn matches_aggregate(
        &self,
        aggregate: &crate::api::AutomaticContinuationAggregateView,
    ) -> bool {
        match (self, aggregate) {
            (
                Self::Ordinary {
                    product_conversation_id,
                },
                crate::api::AutomaticContinuationAggregateView::Ordinary {
                    product_conversation_id: actual,
                },
            ) => product_conversation_id.as_str() == actual,
            (
                Self::Coordinator {},
                crate::api::AutomaticContinuationAggregateView::Coordinator {
                    product_conversation_id,
                },
            ) => ProductConversationId::parse(product_conversation_id).is_ok(),
            _ => false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteAutomaticContinuationReadRequest {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub target: RemoteAutomaticContinuationTarget,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteAutomaticContinuationSetRequest {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub target: RemoteAutomaticContinuationTarget,
    pub auto_continue_on_context_exhaustion: bool,
}

impl RemoteAutomaticContinuationReadRequest {
    async fn verify(
        &self,
        peer: &PeerAuthenticated,
        db: &crate::db::Database,
    ) -> Result<InstanceId, Response> {
        let destination = db.instance_id().await.map_err(|error| {
            tracing::error!(%error, "failed to read destination instance identity");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?;
        if self.destination_instance_id != destination {
            return Err(federation_error(
                StatusCode::CONFLICT,
                "destination instance mismatch",
            ));
        }
        if self.caller_instance_id != peer.caller_instance_id {
            return Err(federation_error(
                StatusCode::UNAUTHORIZED,
                "caller instance mismatch",
            ));
        }
        Ok(destination)
    }
}

fn automatic_continuation_response(
    result: phoenix_db::DbResult<phoenix_db::AutomaticContinuationSettings>,
    destination: InstanceId,
    caller: InstanceId,
) -> Response {
    result.map_or_else(
        |error| {
            if matches!(error, phoenix_db::DbError::FederationSendAuthorityInactive) {
                federation_error(
                    StatusCode::FORBIDDEN,
                    "Federation send authority is inactive.",
                )
            } else {
                super::product_conversations::db_to_app(error).into_response()
            }
        },
        |settings| {
            Json(RemoteAutomaticContinuationResponse {
                destination_instance_id: destination,
                caller_instance_id: caller,
                preference: settings.into(),
            })
            .into_response()
        },
    )
}

fn verify_automatic_continuation_response(
    response: RemoteAutomaticContinuationResponse,
    peer_id: InstanceId,
    caller: InstanceId,
    target: &RemoteAutomaticContinuationTarget,
) -> Result<crate::automatic_continuation_settings::native::PreferenceView, FederationClientError> {
    if response.destination_instance_id != peer_id {
        return Err(FederationClientError::DestinationMismatch);
    }
    if response.caller_instance_id != caller {
        return Err(FederationClientError::CallerMismatch);
    }
    if !target.matches_aggregate(&response.preference.aggregate) {
        return Err(FederationClientError::ProvenanceMismatch(
            "automatic-continuation response changed target".into(),
        ));
    }
    Ok(response.preference)
}

async fn automatic_continuation_peer(
    db: &crate::db::Database,
    selector: &str,
) -> Result<(InstanceId, InstanceId, phoenix_db::FederationPeerConnection), FederationClientError> {
    let peer_id = resolve_saved_peer_id(db, selector).await?;
    let caller = db.instance_id().await?;
    if peer_id == caller {
        return Err(FederationClientError::SelfPeerSelector);
    }
    let peer = db
        .federation_peer_connection(peer_id)
        .await?
        .ok_or(FederationClientError::PeerNotFound)?;
    Ok((peer_id, caller, peer))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteAutomaticContinuationResponse {
    pub destination_instance_id: InstanceId,
    pub caller_instance_id: InstanceId,
    pub preference: crate::automatic_continuation_settings::native::PreferenceView,
}

pub async fn remote_read_automatic_continuation(
    db: &crate::db::Database,
    selector: &str,
    target: RemoteAutomaticContinuationTarget,
) -> Result<crate::automatic_continuation_settings::native::PreferenceView, FederationClientError> {
    let (peer_id, caller, peer) = automatic_continuation_peer(db, selector).await?;
    let request = RemoteAutomaticContinuationReadRequest {
        destination_instance_id: peer_id,
        caller_instance_id: caller,
        target,
    };
    let response: RemoteAutomaticContinuationResponse = send_bounded_peer_request(
        format!(
            "{}api/federation/peer/read-automatic-continuation",
            peer.base_url
        ),
        peer.bearer_credential.expose(),
        &peer.tls_trust,
        &request,
    )
    .await?;
    verify_automatic_continuation_response(response, peer_id, caller, &request.target)
}

pub async fn remote_set_automatic_continuation(
    db: &crate::db::Database,
    selector: &str,
    target: RemoteAutomaticContinuationTarget,
    preference: bool,
) -> Result<crate::automatic_continuation_settings::native::PreferenceView, FederationClientError> {
    let (peer_id, caller, peer) = automatic_continuation_peer(db, selector).await?;
    let request = RemoteAutomaticContinuationSetRequest {
        destination_instance_id: peer_id,
        caller_instance_id: caller,
        target,
        auto_continue_on_context_exhaustion: preference,
    };
    let response: RemoteAutomaticContinuationResponse = send_bounded_peer_request(
        format!(
            "{}api/federation/peer/set-automatic-continuation",
            peer.base_url
        ),
        peer.bearer_credential.expose(),
        &peer.tls_trust,
        &request,
    )
    .await?;
    let result =
        verify_automatic_continuation_response(response, peer_id, caller, &request.target)?;
    if result.auto_continue_on_context_exhaustion != preference {
        return Err(FederationClientError::ProvenanceMismatch(
            "automatic-continuation response changed preference".into(),
        ));
    }
    Ok(result)
}

async fn peer_authority_response(
    runtime: &crate::runtime::RuntimeManager,
    response: Response,
) -> Response {
    if runtime.local_authority_is_closed() {
        std::future::pending().await
    } else {
        response
    }
}

async fn peer_write_join_response(
    runtime: &crate::runtime::RuntimeManager,
    result: Result<Response, tokio::task::JoinError>,
) -> Response {
    match result {
        Ok(response) => peer_authority_response(runtime, response).await,
        Err(error) if runtime.local_authority_is_closed() => {
            tracing::debug!(%error, "federated automatic-continuation write ended after local authority closure");
            std::future::pending().await
        }
        Err(error) => {
            tracing::error!(%error, "federated automatic-continuation write task failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn read_automatic_continuation(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteAutomaticContinuationReadRequest>,
) -> Response {
    if state.runtime.local_authority_is_closed() {
        return std::future::pending().await;
    }
    let destination = match request.verify(&peer, &state.db).await {
        Ok(destination) => destination,
        Err(response) => {
            if state.runtime.local_authority_is_closed() {
                return std::future::pending().await;
            }
            return response;
        }
    };
    let Ok(_authority) = state.runtime.acquire_local_authority_pass() else {
        return std::future::pending().await;
    };
    let result = state
        .db
        .read_federated_automatic_continuation_settings(
            &peer.send_authority,
            request.target.db_target(),
        )
        .await;
    peer_authority_response(
        &state.runtime,
        automatic_continuation_response(result, destination, peer.caller_instance_id),
    )
    .await
}

async fn verify_set_automatic_continuation_identity(
    runtime: &crate::runtime::RuntimeManager,
    peer: &PeerAuthenticated,
    request: &RemoteAutomaticContinuationSetRequest,
    identity: impl std::future::Future<Output = phoenix_db::DbResult<InstanceId>>,
) -> Result<InstanceId, Response> {
    let destination = match identity.await {
        Ok(destination) => destination,
        Err(error) => {
            tracing::error!(%error, "failed to read destination instance identity");
            return Err(peer_authority_response(
                runtime,
                StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            )
            .await);
        }
    };
    if request.destination_instance_id != destination {
        return Err(peer_authority_response(
            runtime,
            federation_error(StatusCode::CONFLICT, "destination instance mismatch"),
        )
        .await);
    }
    if request.caller_instance_id != peer.caller_instance_id {
        return Err(peer_authority_response(
            runtime,
            federation_error(StatusCode::UNAUTHORIZED, "caller instance mismatch"),
        )
        .await);
    }
    Ok(destination)
}

pub async fn set_automatic_continuation(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<RemoteAutomaticContinuationSetRequest>,
) -> Response {
    if state.runtime.local_authority_is_closed() {
        return std::future::pending().await;
    }
    let destination = match verify_set_automatic_continuation_identity(
        &state.runtime,
        &peer,
        &request,
        state.db.instance_id(),
    )
    .await
    {
        Ok(destination) => destination,
        Err(response) => return response,
    };
    let db = state.db.clone();
    let runtime = state.runtime.clone();
    if state.runtime.local_authority_is_closed() {
        return std::future::pending().await;
    }

    let operation = async move {
        let Ok(authority) = runtime.acquire_local_authority_pass() else {
            return std::future::pending().await;
        };
        let pass =
            crate::automatic_continuation_settings::native::PreferenceWritePass::new(authority);
        let result = db
            .set_federated_automatic_continuation_settings(
                &peer.send_authority,
                request.target.db_target(),
                request.auto_continue_on_context_exhaustion.into(),
            )
            .await;
        let result = pass.classify(result);
        if runtime.local_authority_is_closed() {
            return std::future::pending().await;
        }
        match result {
            Ok(result) => {
                automatic_continuation_response(result, destination, peer.caller_instance_id)
            }
            Err(error) => {
                tracing::error!(%error, "federated automatic-continuation setting durability unclassified");
                std::future::pending().await
            }
        }
    };
    peer_write_join_response(&state.runtime, tokio::spawn(operation).await).await
}

async fn resolve_saved_peer_id(
    db: &crate::db::Database,
    selector: &str,
) -> Result<InstanceId, FederationClientError> {
    if selector.is_empty() {
        return Err(FederationClientError::InvalidPeerSelector);
    }
    if let Ok(id) = InstanceId::from_str(selector) {
        if selector == id.to_string() && db.federation_peer_connection(id).await?.is_some() {
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

pub(crate) fn validate_remote_resolve_reference(
    reference: &str,
) -> Result<RemoteConversationTarget, FederationClientError> {
    RemoteConversationTarget::parse(reference)
}

pub(crate) fn conversation_lookup_identity(
    reference: &str,
) -> Result<String, FederationClientError> {
    if reference.starts_with("@conv:") || reference.starts_with("@transcript:") {
        return validate_remote_resolve_reference(reference)
            .map(|target| target.lookup_identity().to_owned());
    }
    if reference.trim().is_empty()
        || reference.trim() != reference
        || reference.len() > NATIVE_RENAME_REQUEST_BYTES
        || reference.chars().any(char::is_control)
    {
        return Err(FederationClientError::ProvenanceMismatch(
            "reference must be one bounded nonblank identity or canonical typed reference"
                .to_string(),
        ));
    }
    Ok(reference.to_owned())
}

pub async fn resolve_remote_reference(
    db: &crate::db::Database,
    peer: &ResolvedPeer,
    reference: &str,
) -> Result<RemoteResolveReferenceResponse, FederationClientError> {
    let requested = validate_remote_resolve_reference(reference)?;
    let peer_id = peer.instance_id();
    let caller = db.instance_id().await?;
    let mut response: RemoteResolveReferenceResponse = send_bounded_peer_request(
        format!(
            "{}api/federation/peer/resolve-reference",
            peer.connection.base_url
        ),
        peer.connection.bearer_credential.expose(),
        &peer.connection.tls_trust,
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

fn validate_remote_read_cursor(cursor: usize) -> Result<(), FederationClientError> {
    if cursor > super::global_read::MAX_CONVERSATION_READ_CURSOR {
        return Err(FederationClientError::RemoteRejected(format!(
            "cursor exceeds maximum {}",
            super::global_read::MAX_CONVERSATION_READ_CURSOR
        )));
    }
    Ok(())
}

pub(crate) fn validate_remote_read_target(
    reference: &str,
    cursor: usize,
) -> Result<RemoteConversationTarget, FederationClientError> {
    validate_remote_read_cursor(cursor)?;
    let requested = RemoteConversationTarget::parse(reference)?;
    require_bounded_read_target(&requested)?;
    validate_remote_cursor_target(&requested, cursor)
        .map_err(|error| FederationClientError::RemoteRejected(error.to_string()))?;
    Ok(requested)
}

pub async fn read_remote_conversation(
    db: &crate::db::Database,
    peer: &ResolvedPeer,
    reference: &str,
    cursor: usize,
) -> Result<RemoteReadConversationResponse, FederationClientError> {
    let requested = validate_remote_read_target(reference, cursor)?;
    let peer_id = peer.instance_id();
    let caller = db.instance_id().await?;
    let request = RemoteReadConversationRequest {
        destination_instance_id: peer_id,
        target: requested.clone(),
        cursor,
    };
    let response: RemoteReadConversationResponse = send_bounded_peer_request(
        format!(
            "{}api/federation/peer/read-conversation",
            peer.connection.base_url
        ),
        peer.connection.bearer_credential.expose(),
        &peer.connection.tls_trust,
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
    validate_remote_goal_history(response, request)
}

fn validate_remote_goal_history(
    response: &RemoteReadGoalResponse,
    request: &RemoteReadGoalRequest,
) -> Result<(), FederationClientError> {
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
    let mut previous_entry: Option<&GoalHistoryEntry> = None;
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

    validate_remote_goal_wait_status(response, request)
}

fn validate_remote_goal_wait_status(
    response: &RemoteReadGoalResponse,
    request: &RemoteReadGoalRequest,
) -> Result<(), FederationClientError> {
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
    peer: &ResolvedPeer,
    target: &ProductConversationId,
    after_version: GoalVersion,
    wait: Option<CoordinatorGoalWaitRequest>,
) -> Result<RemoteReadGoalResponse, FederationClientError> {
    let peer_id = peer.instance_id();
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
        format!("{}api/federation/peer/read-goal", peer.connection.base_url),
        peer.connection.bearer_credential.expose(),
        &peer.connection.tls_trust,
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
        GoalDbError::Invalid(message) => federation_error(StatusCode::BAD_REQUEST, message),
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
    if request.caller_instance_id == local_id {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    let token = random_peer_token();
    let verifier = FederationCredentialVerifier::from_bearer(token.as_bytes());
    if let Err(error) = state
        .db
        .replace_federation_enrollment(
            request.caller_instance_id,
            &request.caller_display_name,
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
            &request.peer_display_name,
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

fn native_canonical_uuid<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = String::deserialize(deserializer)?;
    if !native_uuid_is_canonical(&value) {
        return Err(serde::de::Error::custom(
            "expected canonical lowercase hyphenated UUID",
        ));
    }
    value.parse().map_err(serde::de::Error::custom)
}

fn native_uuid_is_canonical(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeCreationState {
    Accepted,
    Claimed,
    RetryScheduled,
    Cancelling,
    Cancelled,
    DeletionPending,
    DeliveryPending,
    DeliveryFailed,
    Published,
    Failed,
    CleanupAmbiguous,
}

pub(crate) const NATIVE_CREATE_OPTIONS_REQUEST_BYTES: usize = 4096;
pub(crate) const NATIVE_CREATE_OPTIONS_RESPONSE_BYTES: usize = 262_144;
pub(crate) const NATIVE_CREATE_REQUEST_BYTES: usize = 65_536;
pub(crate) const NATIVE_CREATE_RESPONSE_BYTES: usize = 4096;
pub(crate) const NATIVE_CREATE_STATUS_REQUEST_BYTES: usize = 4096;
pub(crate) const NATIVE_CREATE_STATUS_RESPONSE_BYTES: usize = 16_384;
pub(crate) const NATIVE_RENAME_OPTIONS_REQUEST_BYTES: usize = 4096;
pub(crate) const NATIVE_RENAME_OPTIONS_RESPONSE_BYTES: usize = 262_144;
pub(crate) const NATIVE_RENAME_REQUEST_BYTES: usize = 16_384;
pub(crate) const NATIVE_RENAME_RESPONSE_BYTES: usize = 16_384;
const NATIVE_DOMAIN_ID_MAX_BYTES: usize = 256;
const NATIVE_CREATE_PROJECT_LIMIT: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum NativeCreateOptionsRequest {
    WithoutCwd {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        correlation_id: String,
    },
    WithCwd {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        correlation_id: String,
        cwd: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRenameOptionsRequest {
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) caller_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) destination_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) correlation_id: String,
    pub(crate) reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeActionSourceCall {
    #[serde(rename = "source_product_conversation_id")]
    pub(crate) product_conversation: ProductConversationId,
    pub(crate) source_transcript_id: String,
    pub(crate) source_assistant_message_id: String,
    pub(crate) source_tool_use_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeCreateRequest {
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) caller_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) destination_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) request_id: String,
    pub(crate) cwd: String,
    pub(crate) objective: String,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub(crate) model: Option<String>,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub(crate) effort: Option<String>,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub(crate) language: Option<String>,
    pub(crate) start: NativeCreateStart,
    pub(crate) source: NativeActionSourceCall,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeCreateStart {
    SubmitObjective,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeCreateStatusRequest {
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) caller_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) destination_instance_id: InstanceId,
    #[serde(rename = "request_id", deserialize_with = "native_canonical_uuid")]
    pub(crate) request: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRenameRequest {
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) caller_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) destination_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) operation_id: String,
    pub(crate) product_conversation_id: ProductConversationId,
    pub(crate) expected_root_transcript_id: String,
    pub(crate) expected_name_revision: i64,
    pub(crate) title: String,
    pub(crate) source: NativeActionSourceCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeCreateProjectOption {
    pub(crate) project_id: String,
    pub(crate) cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeCreateModelOption {
    pub(crate) model: String,
    #[serde(deserialize_with = "deserialize_effort_capabilities")]
    pub(crate) supported_efforts: phoenix_llm::EffortCapabilities,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NativeDefaultWire {
    Known(NativeDefaultKnownWire),
    Unknown(NativeDefaultUnknownWire),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeDefaultKnownWire {
    known: phoenix_core::domain::llm_types::ModelEffort,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum NativeDefaultUnknownWire {
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "support", rename_all = "snake_case", deny_unknown_fields)]
enum EffortCapabilitiesWire {
    Unsupported {},
    Unknown {},
    Supported {
        levels: Vec<phoenix_core::domain::llm_types::ModelEffort>,
        native_default: NativeDefaultWire,
    },
}

fn deserialize_effort_capabilities<'de, D>(
    deserializer: D,
) -> Result<phoenix_llm::EffortCapabilities, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match EffortCapabilitiesWire::deserialize(deserializer)? {
        EffortCapabilitiesWire::Unsupported {} => Ok(phoenix_llm::EffortCapabilities::Unsupported),
        EffortCapabilitiesWire::Unknown {} => Ok(phoenix_llm::EffortCapabilities::Unknown),
        EffortCapabilitiesWire::Supported {
            levels,
            native_default,
        } => {
            if levels.is_empty() {
                return Err(serde::de::Error::custom(
                    "supported effort capabilities require at least one level",
                ));
            }
            let mut unique_levels = std::collections::HashSet::new();
            if levels
                .iter()
                .any(|level| !unique_levels.insert(level.as_wire_name()))
            {
                return Err(serde::de::Error::custom(
                    "supported effort capabilities contain duplicate levels",
                ));
            }
            let native_default = match native_default {
                NativeDefaultWire::Known(NativeDefaultKnownWire { known })
                    if !levels.contains(&known) =>
                {
                    return Err(serde::de::Error::custom(
                        "native effort default must be a supported level",
                    ));
                }
                NativeDefaultWire::Known(NativeDefaultKnownWire { known }) => {
                    phoenix_llm::NativeDefault::Known(known)
                }
                NativeDefaultWire::Unknown(NativeDefaultUnknownWire::Unknown) => {
                    phoenix_llm::NativeDefault::Unknown
                }
            };
            Ok(phoenix_llm::EffortCapabilities::supported(
                &levels,
                native_default,
            ))
        }
    }
}

fn effort_levels(
    capabilities: &phoenix_llm::EffortCapabilities,
) -> Option<&[phoenix_core::domain::llm_types::ModelEffort]> {
    match capabilities {
        phoenix_llm::EffortCapabilities::Supported(supported) => Some(supported.levels()),
        phoenix_llm::EffortCapabilities::Unsupported | phoenix_llm::EffortCapabilities::Unknown => {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeCreateOptionsResponse {
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) caller_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) destination_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) correlation_id: String,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub(crate) cwd: Option<String>,
    pub(crate) projects: Vec<NativeCreateProjectOption>,
    pub(crate) projects_truncated: bool,
    pub(crate) models: Vec<NativeCreateModelOption>,
    pub(crate) default_model: String,
    pub(crate) default_llm_language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRenameDetail {
    pub(crate) can_rename: bool,
    pub(crate) product_conversation_id: ProductConversationId,
    pub(crate) root_transcript_id: String,
    pub(crate) name_revision: i64,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub(crate) title: Option<String>,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub(crate) slug: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeRenameDetailAction {
    Detail,
    Read,
    Rename,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRenameOptionsResponse {
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) caller_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) destination_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) correlation_id: String,
    pub(crate) canonical_route: String,
    pub(crate) detail: NativeRenameDetail,
    pub(crate) allowed_actions: Vec<NativeRenameDetailAction>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeCreateAcceptanceStatus {
    Accepted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeCreateResponse {
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) caller_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) destination_instance_id: InstanceId,
    #[serde(deserialize_with = "native_canonical_uuid")]
    pub(crate) request_id: String,
    pub(crate) product_conversation_id: ProductConversationId,
    pub(crate) status: NativeCreateAcceptanceStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum NativeCreatePublication {
    None {},
    Available {
        conversation_id: String,
        canonical_route: String,
    },
    Historical {
        conversation_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum NativeCreateInitialExecution {
    Queued {},
    Running {},
    Completed {},
    Failed { reason: String },
    Cancelled {},
    AwaitingUserResponse {},
    AwaitingTaskApproval {},
    Unavailable {},
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum NativeCreateStatusResponse {
    Unknown {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        request_id: String,
    },
    Known {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        request_id: String,
        product_conversation_id: ProductConversationId,
        creation_status: NativeCreationState,
        provisioning_stage: phoenix_db::ProductCreationProvisioningStage,
        provisioning_stage_updated_at_unix_micros: i64,
        #[serde(deserialize_with = "deserialize_present_option")]
        retry_at_unix_micros: Option<i64>,
        #[serde(deserialize_with = "deserialize_present_option")]
        error: Option<String>,
        publication: NativeCreatePublication,
        initial_execution: NativeCreateInitialExecution,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum NativeRenameResponse {
    Applied {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        operation_id: String,
        product_conversation_id: ProductConversationId,
        root_transcript_id: String,
        receipt_title: String,
        receipt_slug_at_commit: String,
        receipt_name_revision_before: i64,
        receipt_committed_at_unix_micros: i64,
        #[serde(deserialize_with = "native_canonical_uuid")]
        receipt_source_instance_id: InstanceId,
        receipt_source: NativeActionSourceCall,
    },
    Replayed {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        operation_id: String,
        product_conversation_id: ProductConversationId,
        root_transcript_id: String,
        receipt_title: String,
        receipt_slug_at_commit: String,
        receipt_name_revision_before: i64,
        receipt_committed_at_unix_micros: i64,
        #[serde(deserialize_with = "native_canonical_uuid")]
        receipt_source_instance_id: InstanceId,
        receipt_source: NativeActionSourceCall,
    },
    Stale {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        operation_id: String,
        product_conversation_id: ProductConversationId,
        root_transcript_id: String,
    },
    Conflict {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        operation_id: String,
        product_conversation_id: ProductConversationId,
        root_transcript_id: String,
    },
    NotFound {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        operation_id: String,
        product_conversation_id: ProductConversationId,
        root_transcript_id: String,
    },
    Unavailable {
        #[serde(deserialize_with = "native_canonical_uuid")]
        caller_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        destination_instance_id: InstanceId,
        #[serde(deserialize_with = "native_canonical_uuid")]
        operation_id: String,
        product_conversation_id: ProductConversationId,
        root_transcript_id: String,
    },
}

fn native_action_source(
    source: &phoenix_db::ProductConversationRenameReceiptSource,
) -> Result<NativeActionSourceCall, NativeActionFailure> {
    Ok(NativeActionSourceCall {
        product_conversation: ProductConversationId::parse(source.product_conversation_id.clone())
            .map_err(native_error)?,
        source_transcript_id: source.transcript_id.clone(),
        source_assistant_message_id: source.message_id.clone(),
        source_tool_use_id: source.tool_use_id.clone(),
    })
}

fn native_initial_execution(
    execution: &crate::db::ProductCreationInitialExecution,
) -> NativeCreateInitialExecution {
    use crate::db::ProductCreationInitialExecution as Execution;
    match execution {
        Execution::Queued => NativeCreateInitialExecution::Queued {},
        Execution::Running => NativeCreateInitialExecution::Running {},
        Execution::Completed => NativeCreateInitialExecution::Completed {},
        Execution::Failed { reason } => NativeCreateInitialExecution::Failed {
            reason: reason.clone(),
        },
        Execution::Cancelled => NativeCreateInitialExecution::Cancelled {},
        Execution::AwaitingUserResponse => NativeCreateInitialExecution::AwaitingUserResponse {},
        Execution::AwaitingTaskApproval => NativeCreateInitialExecution::AwaitingTaskApproval {},
        Execution::Unavailable => NativeCreateInitialExecution::Unavailable {},
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeActionErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    TooLarge,
    Unavailable,
    Internal,
    UnsupportedEndpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeActionErrorBody {
    pub(crate) error_type: NativeActionErrorCode,
    pub(crate) error: String,
}

#[derive(Debug)]
pub(crate) struct NativeActionFailure {
    status: StatusCode,
    body: NativeActionErrorBody,
}

impl NativeActionFailure {
    fn new(status: StatusCode, code: NativeActionErrorCode) -> Self {
        let error = match code {
            NativeActionErrorCode::InvalidRequest => "invalid native action request",
            NativeActionErrorCode::Unauthorized => "native action authentication required",
            NativeActionErrorCode::Forbidden => "native action is not authorized",
            NativeActionErrorCode::NotFound => "native action target was not found",
            NativeActionErrorCode::Conflict => "native action conflicts with destination state",
            NativeActionErrorCode::TooLarge => "native action payload is too large",
            NativeActionErrorCode::Unavailable => "native action is unavailable",
            NativeActionErrorCode::Internal => "native action failed",
            NativeActionErrorCode::UnsupportedEndpoint => "native action endpoint is unsupported",
        };
        Self {
            status,
            body: NativeActionErrorBody {
                error_type: code,
                error: error.into(),
            },
        }
    }
}

impl From<StatusCode> for NativeActionFailure {
    fn from(status: StatusCode) -> Self {
        let code = match status {
            StatusCode::BAD_REQUEST
            | StatusCode::UNPROCESSABLE_ENTITY
            | StatusCode::UNSUPPORTED_MEDIA_TYPE => NativeActionErrorCode::InvalidRequest,
            StatusCode::UNAUTHORIZED => NativeActionErrorCode::Unauthorized,
            StatusCode::FORBIDDEN => NativeActionErrorCode::Forbidden,
            StatusCode::NOT_FOUND => NativeActionErrorCode::NotFound,
            StatusCode::CONFLICT => NativeActionErrorCode::Conflict,
            StatusCode::PAYLOAD_TOO_LARGE => NativeActionErrorCode::TooLarge,
            StatusCode::SERVICE_UNAVAILABLE => NativeActionErrorCode::Unavailable,
            _ => NativeActionErrorCode::Internal,
        };
        Self::new(status, code)
    }
}

impl IntoResponse for NativeActionFailure {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

fn native_json_rejection(error: &axum::extract::rejection::JsonRejection) -> NativeActionFailure {
    NativeActionFailure::from(error.status())
}

fn native_source_valid(source: &NativeActionSourceCall) -> bool {
    native_domain_identity_valid(source.product_conversation.as_str())
        && native_domain_identity_valid(&source.source_transcript_id)
        && native_domain_identity_valid(&source.source_assistant_message_id)
        && native_domain_identity_valid(&source.source_tool_use_id)
}

fn native_domain_identity_valid(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= NATIVE_DOMAIN_ID_MAX_BYTES
        && !value.chars().any(char::is_control)
}

fn native_create_intent_valid(request: &NativeCreateRequest) -> bool {
    native_uuid_is_canonical(&request.request_id)
        && !request.cwd.trim().is_empty()
        && !request.cwd.chars().any(char::is_control)
        && !request.objective.trim().is_empty()
        && native_source_valid(&request.source)
        && [&request.model, &request.effort, &request.language]
            .iter()
            .all(|value| {
                value
                    .as_ref()
                    .is_none_or(|v| !v.trim().is_empty() && !v.chars().any(char::is_control))
            })
}

fn native_rename_intent_valid(request: &NativeRenameRequest) -> bool {
    native_uuid_is_canonical(&request.operation_id)
        && native_domain_identity_valid(request.product_conversation_id.as_str())
        && native_domain_identity_valid(&request.expected_root_transcript_id)
        && (0..i64::MAX).contains(&request.expected_name_revision)
        && native_source_valid(&request.source)
        && crate::conversation_actions::normalize_product_conversation_title(&request.title).is_ok()
}

fn native_rename_receipt_responses_fit(request: &NativeRenameRequest) -> bool {
    const MAX_LOWERCASE_UTF8_BYTES_PER_INPUT_CHAR: usize = 12;
    const MAX_COLLISION_SUFFIX_BYTES: usize = 21;

    let max_slug = "x".repeat(
        request
            .title
            .chars()
            .count()
            .saturating_mul(MAX_LOWERCASE_UTF8_BYTES_PER_INPUT_CHAR)
            .saturating_add(MAX_COLLISION_SUFFIX_BYTES),
    );
    let applied = NativeRenameResponse::Applied {
        caller_instance_id: request.caller_instance_id,
        destination_instance_id: request.destination_instance_id,
        operation_id: request.operation_id.clone(),
        product_conversation_id: request.product_conversation_id.clone(),
        root_transcript_id: request.expected_root_transcript_id.clone(),
        receipt_title: request.title.clone(),
        receipt_slug_at_commit: max_slug.clone(),
        receipt_name_revision_before: request.expected_name_revision,
        receipt_committed_at_unix_micros: i64::MIN,
        receipt_source_instance_id: request.caller_instance_id,
        receipt_source: request.source.clone(),
    };
    let replayed = NativeRenameResponse::Replayed {
        caller_instance_id: request.caller_instance_id,
        destination_instance_id: request.destination_instance_id,
        operation_id: request.operation_id.clone(),
        product_conversation_id: request.product_conversation_id.clone(),
        root_transcript_id: request.expected_root_transcript_id.clone(),
        receipt_title: request.title.clone(),
        receipt_slug_at_commit: max_slug,
        receipt_name_revision_before: request.expected_name_revision,
        receipt_committed_at_unix_micros: i64::MIN,
        receipt_source_instance_id: request.caller_instance_id,
        receipt_source: request.source.clone(),
    };
    [applied, replayed].iter().all(|response| {
        serde_json::to_vec(response)
            .is_ok_and(|bytes| !bytes.is_empty() && bytes.len() <= NATIVE_RENAME_RESPONSE_BYTES)
    })
}

fn native_peer_authority(
    peer: PeerAuthenticated,
    caller: InstanceId,
    destination: InstanceId,
    local: InstanceId,
) -> Result<phoenix_db::FederationConversationAuthority, NativeActionFailure> {
    if peer.caller_instance_id != caller {
        return Err(StatusCode::UNAUTHORIZED.into());
    }
    if destination != local {
        return Err(StatusCode::CONFLICT.into());
    }
    Ok(peer.send_authority.into())
}

fn native_json<T: Serialize>(body: &T, max_bytes: usize) -> Result<Response, NativeActionFailure> {
    let bytes = serde_json::to_vec(body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
    }
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        bytes,
    )
        .into_response())
}

fn native_error(error: impl std::fmt::Display) -> NativeActionFailure {
    tracing::warn!(%error, "native federation action failed");
    NativeActionFailure::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        NativeActionErrorCode::Internal,
    )
}

fn native_app_error(error: super::handlers::AppError) -> NativeActionFailure {
    NativeActionFailure::from(error.into_response().status())
}

fn normalized_native_cwd(cwd: &str, home: &std::path::Path) -> Result<String, NativeActionFailure> {
    let cwd = cwd.trim();
    let path = if cwd == "~" {
        home.to_path_buf()
    } else if let Some(suffix) = cwd.strip_prefix("~/") {
        home.join(suffix)
    } else {
        std::path::PathBuf::from(cwd)
    };
    if cwd.is_empty() || !path.is_dir() {
        return Err(StatusCode::BAD_REQUEST.into());
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| StatusCode::BAD_REQUEST)?;
    crate::conversation_cwd::validate_conversation_cwd(canonical.to_string_lossy())
        .map(crate::conversation_cwd::ValidConversationCwd::into_raw)
        .map_err(|_| StatusCode::BAD_REQUEST.into())
}

pub(crate) async fn native_create_options(
    State(state): State<AppState>,
    peer: PeerAuthenticated,
    request: Result<Json<NativeCreateOptionsRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, NativeActionFailure> {
    let Json(request) = request.map_err(|error| native_json_rejection(&error))?;
    let (caller, destination, correlation_id, cwd) = match request {
        NativeCreateOptionsRequest::WithoutCwd {
            caller_instance_id,
            destination_instance_id,
            correlation_id,
        } => (
            caller_instance_id,
            destination_instance_id,
            correlation_id,
            None,
        ),
        NativeCreateOptionsRequest::WithCwd {
            caller_instance_id,
            destination_instance_id,
            correlation_id,
            cwd,
        } => (
            caller_instance_id,
            destination_instance_id,
            correlation_id,
            Some(cwd),
        ),
    };
    let local = state.db.instance_id().await.map_err(native_error)?;
    let authority = native_peer_authority(peer, caller, destination, local)?;
    let cwd = match cwd {
        Some(cwd) => {
            let home = state.runtime.runtime_home().to_path_buf();
            Some(
                tokio::task::spawn_blocking(move || normalized_native_cwd(&cwd, &home))
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??,
            )
        }
        None => None,
    };
    let (projects, truncated) = native_create_projects(&state, &authority).await?;
    let registry = state.runtime.model_registry();
    let models = registry
        .available_execution_routes()
        .into_iter()
        .map(|route| NativeCreateModelOption {
            model: route.model,
            supported_efforts: route.supported_efforts,
        })
        .collect();
    let default_llm_language = state
        .db
        .get_default_llm_language()
        .await
        .map_err(native_error)?;
    native_json(
        &NativeCreateOptionsResponse {
            caller_instance_id: caller,
            destination_instance_id: destination,
            correlation_id,
            cwd,
            projects,
            projects_truncated: truncated,
            models,
            default_model: registry.default_model_id().clone(),
            default_llm_language: default_llm_language.as_str().to_string(),
        },
        NATIVE_CREATE_OPTIONS_RESPONSE_BYTES,
    )
}

async fn native_create_projects(
    state: &AppState,
    authority: &phoenix_db::FederationConversationAuthority,
) -> Result<(Vec<NativeCreateProjectOption>, bool), NativeActionFailure> {
    let mut projects = Vec::new();
    let mut cursor = None::<String>;
    while projects.len() <= NATIVE_CREATE_PROJECT_LIMIT {
        let rows = state
            .db
            .product_creation_project_options_as_federation(authority, cursor.as_deref())
            .await
            .map_err(native_error)?;
        let Some((last, _)) = rows.last() else {
            break;
        };
        cursor = Some(last.clone());
        let home = state.runtime.runtime_home().to_path_buf();
        let usable = tokio::task::spawn_blocking(move || {
            rows.into_iter()
                .filter_map(|(project_id, path)| {
                    if let Ok(cwd) = normalized_native_cwd(&path, &home) {
                        Some(NativeCreateProjectOption { project_id, cwd })
                    } else {
                        tracing::debug!(
                            %project_id,
                            %path,
                            "omitting unusable creation project option"
                        );
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        projects.extend(
            usable
                .into_iter()
                .take(NATIVE_CREATE_PROJECT_LIMIT + 1 - projects.len()),
        );
    }
    let truncated = projects.len() > NATIVE_CREATE_PROJECT_LIMIT;
    projects.truncate(NATIVE_CREATE_PROJECT_LIMIT);
    Ok((projects, truncated))
}

pub(crate) async fn native_rename_options(
    State(state): State<AppState>,
    peer: PeerAuthenticated,
    request: Result<Json<NativeRenameOptionsRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, NativeActionFailure> {
    let Json(request) = request.map_err(|error| native_json_rejection(&error))?;
    let lookup_identity = conversation_lookup_identity(&request.reference)
        .map_err(|_| NativeActionFailure::from(StatusCode::BAD_REQUEST))?;
    let local = state.db.instance_id().await.map_err(native_error)?;
    let authority = native_peer_authority(
        peer,
        request.caller_instance_id,
        request.destination_instance_id,
        local,
    )?;
    let detail = state
        .db
        .read_product_conversation_rename_detail_as_federation(&authority, &lookup_identity)
        .await
        .map_err(native_error)?;
    let route = crate::api::product_conversations::product_route(
        detail.slug.as_deref(),
        detail.product_conversation_id.as_str(),
    );
    let allowed_actions = if detail.can_rename {
        vec![
            NativeRenameDetailAction::Detail,
            NativeRenameDetailAction::Rename,
        ]
    } else {
        vec![NativeRenameDetailAction::Detail]
    };
    native_json(
        &NativeRenameOptionsResponse {
            caller_instance_id: request.caller_instance_id,
            destination_instance_id: local,
            correlation_id: request.correlation_id,
            canonical_route: route,
            detail: NativeRenameDetail {
                can_rename: detail.can_rename,
                product_conversation_id: detail.product_conversation_id,
                root_transcript_id: detail.root_transcript_id,
                name_revision: detail.name_revision,
                title: detail.title,
                slug: detail.slug,
            },
            allowed_actions,
        },
        NATIVE_RENAME_OPTIONS_RESPONSE_BYTES,
    )
}

pub(crate) async fn native_create(
    State(state): State<AppState>,
    peer: PeerAuthenticated,
    request: Result<Json<NativeCreateRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let runtime = state.runtime.clone();
    let response = native_create_inner(state, peer, request)
        .await
        .into_response();
    native_create_response(&runtime, response).await
}

async fn native_create_response(
    runtime: &crate::runtime::RuntimeManager,
    response: Response,
) -> Response {
    peer_authority_response(runtime, response).await
}

async fn native_create_inner(
    state: AppState,
    peer: PeerAuthenticated,
    request: Result<Json<NativeCreateRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, NativeActionFailure> {
    let Json(request) = request.map_err(|error| native_json_rejection(&error))?;
    if !native_create_intent_valid(&request) {
        return Err(StatusCode::BAD_REQUEST.into());
    }
    let local = state.db.instance_id().await.map_err(native_error)?;
    let authority = native_peer_authority(
        peer,
        request.caller_instance_id,
        request.destination_instance_id,
        local,
    )?;
    let effort = request
        .effort
        .map(|value| value.parse().map_err(|_| StatusCode::BAD_REQUEST))
        .transpose()?;
    let language = request
        .language
        .map(|value| {
            serde_json::from_value(serde_json::Value::String(value))
                .map_err(|_| StatusCode::BAD_REQUEST)
        })
        .transpose()?;
    let NativeCreateStart::SubmitObjective = request.start;
    let accepted = crate::conversation_actions::accept_create_from_federation(
        &state.runtime,
        authority,
        super::types::CreateProductConversationRequest {
            request_id: request.request_id,
            cwd: request.cwd,
            objective: request.objective,
            model: request.model,
            effort,
            llm_language: language,
            images: Vec::new(),
        },
        request.source.product_conversation,
        request.source.source_transcript_id,
        phoenix_core::domain::db_schema::SourceToolCall {
            message_id: request.source.source_assistant_message_id,
            tool_use_id: request.source.source_tool_use_id,
        },
    )
    .await
    .map_err(native_app_error)?;
    native_json(
        &NativeCreateResponse {
            caller_instance_id: request.caller_instance_id,
            destination_instance_id: local,
            request_id: accepted.request_id,
            product_conversation_id: accepted.product_conversation_id,
            status: NativeCreateAcceptanceStatus::Accepted,
        },
        NATIVE_CREATE_RESPONSE_BYTES,
    )
}

pub(crate) async fn native_create_status(
    State(state): State<AppState>,
    peer: PeerAuthenticated,
    request: Result<Json<NativeCreateStatusRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, NativeActionFailure> {
    let Json(request) = request.map_err(|error| native_json_rejection(&error))?;
    let local = state.db.instance_id().await.map_err(native_error)?;
    let authority = native_peer_authority(
        peer,
        request.caller_instance_id,
        request.destination_instance_id,
        local,
    )?;
    let status = crate::conversation_actions::read_create_status_from_federation(
        &state.runtime,
        &authority,
        &request.request,
    )
    .await
    .map_err(native_app_error)?;
    let mut response = match status {
        None => NativeCreateStatusResponse::Unknown {
            caller_instance_id: request.caller_instance_id,
            destination_instance_id: local,
            request_id: request.request,
        },
        Some(status) => {
            let publication = match status.publication {
                None => NativeCreatePublication::None {},
                Some(phoenix_db::ProductCreationPublicationRead::Available {
                    conversation_id,
                    aggregate,
                }) => NativeCreatePublication::Available {
                    conversation_id,
                    canonical_route: crate::api::product_conversations::canonical_route(&aggregate),
                },
                Some(phoenix_db::ProductCreationPublicationRead::Historical {
                    conversation_id,
                }) => NativeCreatePublication::Historical { conversation_id },
            };
            let initial_execution = native_initial_execution(&status.initial_objective_execution);
            let creation_status = serde_json::from_value(serde_json::Value::String(status.status))
                .map_err(native_error)?;
            NativeCreateStatusResponse::Known {
                caller_instance_id: request.caller_instance_id,
                destination_instance_id: local,
                request_id: request.request,
                product_conversation_id: status.product_conversation_id,
                creation_status,
                provisioning_stage: status.provisioning_stage,
                provisioning_stage_updated_at_unix_micros: status
                    .stage_updated_at
                    .timestamp_micros(),
                retry_at_unix_micros: status.retry_at.map(|at| at.timestamp_micros()),
                error: status.error,
                publication,
                initial_execution,
            }
        }
    };
    native_bound_status_diagnostics(&mut response)?;
    native_json(&response, NATIVE_CREATE_STATUS_RESPONSE_BYTES)
}

fn native_bound_status_diagnostics(
    response: &mut NativeCreateStatusResponse,
) -> Result<(), NativeActionFailure> {
    const OMITTED: &str = "diagnostic omitted: response size limit";
    let fits = |response: &NativeCreateStatusResponse| {
        serde_json::to_vec(response)
            .map(|bytes| bytes.len() <= NATIVE_CREATE_STATUS_RESPONSE_BYTES)
            .map_err(native_error)
    };
    if fits(response)? {
        return Ok(());
    }
    if let NativeCreateStatusResponse::Known {
        error,
        initial_execution,
        ..
    } = response
    {
        if let Some(error) = error {
            if error.len() > NATIVE_CREATE_STATUS_RESPONSE_BYTES / 2 {
                *error = OMITTED.to_string();
            }
        }
        if let NativeCreateInitialExecution::Failed { reason } = initial_execution {
            if reason.len() > NATIVE_CREATE_STATUS_RESPONSE_BYTES / 2 {
                *reason = OMITTED.to_string();
            }
        }
    }
    if !fits(response)? {
        if let NativeCreateStatusResponse::Known {
            error: Some(error), ..
        } = response
        {
            *error = OMITTED.to_string();
        }
    }
    if !fits(response)? {
        if let NativeCreateStatusResponse::Known {
            initial_execution: NativeCreateInitialExecution::Failed { reason },
            ..
        } = response
        {
            *reason = OMITTED.to_string();
        }
    }
    Ok(())
}

pub(crate) async fn native_rename(
    State(state): State<AppState>,
    peer: PeerAuthenticated,
    request: Result<Json<NativeRenameRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let response = native_rename_result(&state, peer, request).await;
    publish_native_rename_response(&state.runtime, response).await
}

async fn native_rename_result(
    state: &AppState,
    peer: PeerAuthenticated,
    request: Result<Json<NativeRenameRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, NativeActionFailure> {
    let Json(request) = request.map_err(|error| native_json_rejection(&error))?;
    if !native_rename_intent_valid(&request) {
        return Err(StatusCode::BAD_REQUEST.into());
    }
    if !native_rename_receipt_responses_fit(&request) {
        return Err(StatusCode::PAYLOAD_TOO_LARGE.into());
    }
    let local = state.db.instance_id().await.map_err(native_error)?;
    let authority = native_peer_authority(
        peer,
        request.caller_instance_id,
        request.destination_instance_id,
        local,
    )?;
    let source = phoenix_db::ProductConversationRenameSource {
        source_instance_id: authority.caller_instance_id(),
        product_conversation_id: request.source.product_conversation,
        transcript_id: request.source.source_transcript_id,
        message_id: request.source.source_assistant_message_id,
        tool_use_id: request.source.source_tool_use_id,
    };
    let result = crate::conversation_actions::rename_from_federation(
        &state.runtime,
        &authority,
        source,
        crate::conversation_actions::CoordinatorRenameIntent {
            operation_id: request.operation_id.clone(),
            product_conversation_id: request.product_conversation_id.clone(),
            expected_root_transcript_id: request.expected_root_transcript_id.clone(),
            expected_name_revision: request.expected_name_revision,
            title: request.title,
        },
    )
    .await
    .map_err(native_app_error)?;
    let response = match result {
        phoenix_db::ProductConversationRenameResult::Applied(receipt) => {
            NativeRenameResponse::Applied {
                caller_instance_id: request.caller_instance_id,
                destination_instance_id: local,
                operation_id: request.operation_id,
                product_conversation_id: request.product_conversation_id,
                root_transcript_id: request.expected_root_transcript_id,
                receipt_title: receipt.title,
                receipt_slug_at_commit: receipt.slug_at_commit,
                receipt_name_revision_before: receipt.name_revision_before,
                receipt_committed_at_unix_micros: receipt.committed_at_unix_micros,
                receipt_source_instance_id: receipt.source.source_instance_id,
                receipt_source: native_action_source(&receipt.source)?,
            }
        }
        phoenix_db::ProductConversationRenameResult::Replayed(receipt) => {
            NativeRenameResponse::Replayed {
                caller_instance_id: request.caller_instance_id,
                destination_instance_id: local,
                operation_id: request.operation_id,
                product_conversation_id: request.product_conversation_id,
                root_transcript_id: request.expected_root_transcript_id,
                receipt_title: receipt.title,
                receipt_slug_at_commit: receipt.slug_at_commit,
                receipt_name_revision_before: receipt.name_revision_before,
                receipt_committed_at_unix_micros: receipt.committed_at_unix_micros,
                receipt_source_instance_id: receipt.source.source_instance_id,
                receipt_source: native_action_source(&receipt.source)?,
            }
        }
        phoenix_db::ProductConversationRenameResult::Stale => NativeRenameResponse::Stale {
            caller_instance_id: request.caller_instance_id,
            destination_instance_id: local,
            operation_id: request.operation_id,
            product_conversation_id: request.product_conversation_id,
            root_transcript_id: request.expected_root_transcript_id,
        },
        phoenix_db::ProductConversationRenameResult::Conflict => NativeRenameResponse::Conflict {
            caller_instance_id: request.caller_instance_id,
            destination_instance_id: local,
            operation_id: request.operation_id,
            product_conversation_id: request.product_conversation_id,
            root_transcript_id: request.expected_root_transcript_id,
        },
        phoenix_db::ProductConversationRenameResult::NotFound => NativeRenameResponse::NotFound {
            caller_instance_id: request.caller_instance_id,
            destination_instance_id: local,
            operation_id: request.operation_id,
            product_conversation_id: request.product_conversation_id,
            root_transcript_id: request.expected_root_transcript_id,
        },
        phoenix_db::ProductConversationRenameResult::Unavailable => {
            NativeRenameResponse::Unavailable {
                caller_instance_id: request.caller_instance_id,
                destination_instance_id: local,
                operation_id: request.operation_id,
                product_conversation_id: request.product_conversation_id,
                root_transcript_id: request.expected_root_transcript_id,
            }
        }
    };
    native_json(&response, NATIVE_RENAME_RESPONSE_BYTES)
}

async fn publish_native_rename_response(
    runtime: &crate::runtime::RuntimeManager,
    response: Result<Response, NativeActionFailure>,
) -> Response {
    peer_authority_response(runtime, response.into_response()).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum NativeCreateOutcome {
    Accepted { acceptance: NativeCreateResponse },
    Reconciled { status: NativeCreateStatusResponse },
}

fn native_protocol(message: &str) -> FederationClientError {
    FederationClientError::NativeProtocolRejected(message.to_string())
}

async fn native_client_identity(
    db: &Database,
    peer: &ResolvedPeer,
    caller: InstanceId,
    destination: InstanceId,
) -> Result<(), FederationClientError> {
    if caller != db.instance_id().await?
        || destination != peer.instance_id()
        || caller == destination
    {
        return Err(FederationClientError::NativeIdentityRejected(
            "request identity does not match selected peer/local instance".into(),
        ));
    }
    Ok(())
}

fn native_prepare_post<Q: Serialize + DeserializeOwned>(
    peer: &ResolvedPeer,
    route: &str,
    request: &Q,
    request_limit: usize,
) -> Result<reqwest::RequestBuilder, FederationClientError> {
    let bytes =
        serde_json::to_vec(request).map_err(|_| native_protocol("request encoding failed"))?;
    if bytes.len() > request_limit {
        return Err(FederationClientError::NativeRequestTooLarge(request_limit));
    }
    // Constructed Rust requests must satisfy the same strict wire checks as ingress.
    serde_json::from_slice::<Q>(&bytes)
        .map_err(|_| native_protocol("request witnesses are malformed"))?;
    let client = remote_query_client(&peer.connection.tls_trust)?;
    Ok(client
        .post(format!(
            "{}{}",
            peer.connection.base_url,
            route.trim_start_matches('/')
        ))
        .bearer_auth(peer.connection.bearer_credential.expose())
        .timeout(std::time::Duration::from_secs(15))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(bytes))
}

async fn native_post<Q, R>(
    peer: &ResolvedPeer,
    route: &str,
    request: &Q,
    request_limit: usize,
    response_limit: usize,
) -> Result<R, FederationClientError>
where
    Q: Serialize + DeserializeOwned,
    R: DeserializeOwned,
{
    let post = native_prepare_post(peer, route, request, request_limit)?;
    native_send_post(post, response_limit).await
}

async fn native_send_post<R: DeserializeOwned>(
    post: reqwest::RequestBuilder,
    response_limit: usize,
) -> Result<R, FederationClientError> {
    let mut response = post.send().await?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > response_limit as u64)
    {
        return Err(FederationClientError::ResponseTooLarge(response_limit));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        append_bounded_response_chunk(&mut body, &chunk, response_limit)?;
    }
    if status != reqwest::StatusCode::OK {
        return Err(native_response_error(status, &body));
    }
    serde_json::from_slice(&body)
        .map_err(|_| native_protocol("response schema or canonical witnesses are invalid"))
}

fn native_response_error(status: reqwest::StatusCode, body: &[u8]) -> FederationClientError {
    if let Ok(error) = serde_json::from_slice::<NativeActionErrorBody>(body) {
        if !native_error_status_matches(status, error.error_type) || error.error.trim().is_empty() {
            return native_protocol("error classification disagrees with HTTP status");
        }
        return match error.error_type {
            NativeActionErrorCode::Unauthorized | NativeActionErrorCode::Forbidden => {
                FederationClientError::NativeIdentityRejected(error.error)
            }
            NativeActionErrorCode::UnsupportedEndpoint => {
                FederationClientError::NativeEndpointUnsupported
            }
            NativeActionErrorCode::Unavailable | NativeActionErrorCode::Internal => {
                FederationClientError::NativeRemoteFailure(error.error)
            }
            NativeActionErrorCode::InvalidRequest
            | NativeActionErrorCode::NotFound
            | NativeActionErrorCode::Conflict
            | NativeActionErrorCode::TooLarge => {
                FederationClientError::NativeDomainRejected(error.error)
            }
        };
    }
    match status {
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED => {
            FederationClientError::NativeEndpointUnsupported
        }
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            FederationClientError::NativeIdentityRejected(format!("HTTP {status}"))
        }
        reqwest::StatusCode::PAYLOAD_TOO_LARGE => FederationClientError::NativeDomainRejected(
            "raw request body exceeded destination limit".into(),
        ),
        _ if status.is_server_error() => {
            FederationClientError::NativeRemoteFailure(format!("HTTP {status}"))
        }
        _ => native_protocol("unclassified native endpoint rejection"),
    }
}

fn native_error_status_matches(status: reqwest::StatusCode, code: NativeActionErrorCode) -> bool {
    match code {
        NativeActionErrorCode::InvalidRequest => matches!(status.as_u16(), 400 | 415 | 422),
        NativeActionErrorCode::Unauthorized => status.as_u16() == 401,
        NativeActionErrorCode::Forbidden => status.as_u16() == 403,
        NativeActionErrorCode::NotFound => status.as_u16() == 404,
        NativeActionErrorCode::Conflict => status.as_u16() == 409,
        NativeActionErrorCode::TooLarge => status.as_u16() == 413,
        NativeActionErrorCode::Unavailable => status.as_u16() == 503,
        NativeActionErrorCode::Internal => status.as_u16() == 500,
        NativeActionErrorCode::UnsupportedEndpoint => matches!(status.as_u16(), 404 | 405),
    }
}

fn native_echo_matches(
    caller: InstanceId,
    destination: InstanceId,
    received_caller: InstanceId,
    received_destination: InstanceId,
    witness: &str,
    received_witness: &str,
) -> Result<(), FederationClientError> {
    if caller != received_caller
        || destination != received_destination
        || witness != received_witness
    {
        return Err(native_protocol(
            "response identities/correlation witness do not match request",
        ));
    }
    Ok(())
}

fn native_option_string(value: &str) -> bool {
    !value.trim().is_empty() && value.trim() == value && !value.chars().any(char::is_control)
}

fn native_verify_create_options(
    request: &NativeCreateOptionsRequest,
    response: &NativeCreateOptionsResponse,
) -> Result<(), FederationClientError> {
    let (caller, destination, correlation, with_cwd) = match request {
        NativeCreateOptionsRequest::WithoutCwd {
            caller_instance_id,
            destination_instance_id,
            correlation_id,
        } => (
            *caller_instance_id,
            *destination_instance_id,
            correlation_id,
            false,
        ),
        NativeCreateOptionsRequest::WithCwd {
            caller_instance_id,
            destination_instance_id,
            correlation_id,
            ..
        } => (
            *caller_instance_id,
            *destination_instance_id,
            correlation_id,
            true,
        ),
    };
    native_echo_matches(
        caller,
        destination,
        response.caller_instance_id,
        response.destination_instance_id,
        correlation,
        &response.correlation_id,
    )?;
    let mut names = std::collections::HashSet::new();
    let mut projects = std::collections::HashSet::new();
    if response.cwd.is_some() != with_cwd
        || response
            .cwd
            .as_ref()
            .is_some_and(|cwd| !native_absolute_path(cwd))
        || response.projects.len() > NATIVE_CREATE_PROJECT_LIMIT
        || (response.projects_truncated && response.projects.len() != NATIVE_CREATE_PROJECT_LIMIT)
        || response.projects.iter().any(|project| {
            !native_domain_identity_valid(&project.project_id)
                || !projects.insert(&project.project_id)
                || !native_absolute_path(&project.cwd)
        })
        || !native_option_string(&response.default_model)
        || serde_json::from_value::<phoenix_core::llm_language::LlmLanguage>(
            serde_json::Value::String(response.default_llm_language.clone()),
        )
        .is_err()
    {
        return Err(native_protocol(
            "destination options defaults/cwd/project choices are invalid",
        ));
    }
    for model in &response.models {
        let mut efforts = std::collections::HashSet::new();
        if !native_option_string(&model.model)
            || !names.insert(&model.model)
            || effort_levels(&model.supported_efforts).is_some_and(|levels| {
                levels
                    .iter()
                    .any(|effort| !efforts.insert(effort.as_wire_name()))
            })
        {
            return Err(native_protocol(
                "destination model/effort choices are invalid",
            ));
        }
    }
    Ok(())
}

fn native_absolute_path(path: &str) -> bool {
    // Remote paths may be Windows or Unix; never resolve them on the source host.
    native_option_string(path)
        && (path.starts_with('/')
            || path.starts_with("\\\\")
            || (path.as_bytes().get(1) == Some(&b':')
                && path
                    .as_bytes()
                    .get(2)
                    .is_some_and(|c| matches!(c, b'/' | b'\\'))
                && path.as_bytes()[0].is_ascii_alphabetic()))
}

fn source_native_options_cwd(cwd: &str) -> Result<String, FederationClientError> {
    let cwd = cwd.trim();
    if !native_option_string(cwd) {
        return Err(native_protocol("invalid cwd"));
    }
    Ok(cwd.to_owned())
}

pub(crate) async fn remote_native_create_options(
    db: &Database,
    peer: &ResolvedPeer,
    request: &NativeCreateOptionsRequest,
) -> Result<NativeCreateOptionsResponse, FederationClientError> {
    let (caller, destination) = match request {
        NativeCreateOptionsRequest::WithoutCwd {
            caller_instance_id,
            destination_instance_id,
            ..
        }
        | NativeCreateOptionsRequest::WithCwd {
            caller_instance_id,
            destination_instance_id,
            ..
        } => (*caller_instance_id, *destination_instance_id),
    };
    native_client_identity(db, peer, caller, destination).await?;
    let mut request = request.clone();
    if let NativeCreateOptionsRequest::WithCwd { cwd, .. } = &mut request {
        *cwd = source_native_options_cwd(cwd)?;
    }
    let response = native_post(
        peer,
        "/api/federation/peer/native-create-options",
        &request,
        NATIVE_CREATE_OPTIONS_REQUEST_BYTES,
        NATIVE_CREATE_OPTIONS_RESPONSE_BYTES,
    )
    .await?;
    native_verify_create_options(&request, &response)?;
    Ok(response)
}

fn native_verify_rename_options(
    request: &NativeRenameOptionsRequest,
    response: &NativeRenameOptionsResponse,
) -> Result<(), FederationClientError> {
    native_echo_matches(
        request.caller_instance_id,
        request.destination_instance_id,
        response.caller_instance_id,
        response.destination_instance_id,
        &request.correlation_id,
        &response.correlation_id,
    )?;
    let detail = &response.detail;
    let expected_actions = if detail.can_rename {
        vec![
            NativeRenameDetailAction::Detail,
            NativeRenameDetailAction::Rename,
        ]
    } else {
        vec![NativeRenameDetailAction::Detail]
    };
    if !native_domain_identity_valid(detail.product_conversation_id.as_str())
        || !native_domain_identity_valid(&detail.root_transcript_id)
        || (request.reference.starts_with("@conv:")
            && conversation_lookup_identity(&request.reference)?
                != detail.product_conversation_id.as_str())
        || detail.name_revision < 0
        || (detail.can_rename && detail.name_revision == i64::MAX)
        || response.canonical_route
            != crate::api::product_conversations::product_route(
                detail.slug.as_deref(),
                detail.product_conversation_id.as_str(),
            )
        || response.allowed_actions.len() != expected_actions.len()
        || expected_actions
            .iter()
            .any(|action| !response.allowed_actions.contains(action))
        || detail.slug.as_ref().is_some_and(|slug| {
            !native_option_string(slug)
                || slug.contains('/')
                || slug.contains('?')
                || slug.contains('#')
        })
    {
        return Err(native_protocol(
            "rename-options detail/route/actions are inconsistent",
        ));
    }
    Ok(())
}

pub(crate) async fn remote_native_rename_options(
    db: &Database,
    peer: &ResolvedPeer,
    request: &NativeRenameOptionsRequest,
) -> Result<NativeRenameOptionsResponse, FederationClientError> {
    native_client_identity(
        db,
        peer,
        request.caller_instance_id,
        request.destination_instance_id,
    )
    .await?;
    conversation_lookup_identity(&request.reference)?;
    let response = native_post(
        peer,
        "/api/federation/peer/native-rename-options",
        request,
        NATIVE_RENAME_OPTIONS_REQUEST_BYTES,
        NATIVE_RENAME_OPTIONS_RESPONSE_BYTES,
    )
    .await?;
    native_verify_rename_options(request, &response)?;
    Ok(response)
}

fn native_status_echo(response: &NativeCreateStatusResponse) -> (InstanceId, InstanceId, &str) {
    match response {
        NativeCreateStatusResponse::Unknown {
            caller_instance_id,
            destination_instance_id,
            request_id,
        }
        | NativeCreateStatusResponse::Known {
            caller_instance_id,
            destination_instance_id,
            request_id,
            ..
        } => (*caller_instance_id, *destination_instance_id, request_id),
    }
}

fn native_verify_status(
    request: &NativeCreateStatusRequest,
    response: &NativeCreateStatusResponse,
) -> Result<(), FederationClientError> {
    use phoenix_db::ProductCreationProvisioningStage as Stage;
    use NativeCreationState as S;

    let (caller, destination, witness) = native_status_echo(response);
    native_echo_matches(
        request.caller_instance_id,
        request.destination_instance_id,
        caller,
        destination,
        &request.request,
        witness,
    )?;
    if let NativeCreateStatusResponse::Known {
        product_conversation_id,
        creation_status,
        provisioning_stage,
        provisioning_stage_updated_at_unix_micros,
        retry_at_unix_micros,
        publication,
        initial_execution,
        ..
    } = response
    {
        if !native_domain_identity_valid(product_conversation_id.as_str()) {
            return Err(native_protocol(
                "known create product conversation identity is invalid",
            ));
        }
        native_verify_status_stage(
            *provisioning_stage_updated_at_unix_micros,
            initial_execution,
        )?;
        if (*creation_status == S::RetryScheduled && retry_at_unix_micros.is_none())
            || (retry_at_unix_micros.is_some()
                && !matches!(
                    creation_status,
                    S::RetryScheduled
                        | S::DeliveryPending
                        | S::Cancelling
                        | S::Cancelled
                        | S::DeletionPending
                        | S::CleanupAmbiguous
                ))
            || retry_at_unix_micros.is_some_and(|timestamp| {
                timestamp < 0 || chrono::DateTime::from_timestamp_micros(timestamp).is_none()
            })
        {
            return Err(native_protocol("creation status metadata is inconsistent"));
        }
        if (matches!(publication, NativeCreatePublication::Available { conversation_id, .. }
            | NativeCreatePublication::Historical { conversation_id } if !native_domain_identity_valid(conversation_id)))
            || (matches!(publication, NativeCreatePublication::Available { .. })
                && *creation_status != S::Published)
            || (*creation_status == S::Published
                && matches!(publication, NativeCreatePublication::None {}))
            || (matches!(publication, NativeCreatePublication::Historical { .. })
                && !matches!(
                    creation_status,
                    S::Published | S::DeliveryPending | S::DeliveryFailed
                ))
            || (*creation_status == S::Published
                && *provisioning_stage != Stage::FinalizePublication)
            || (matches!(creation_status, S::DeliveryPending | S::DeliveryFailed)
                && *provisioning_stage != Stage::BootstrapInitialTurn)
            || (*creation_status == S::Accepted && *provisioning_stage != Stage::ValidateIntent)
            || (matches!(creation_status, S::Claimed | S::RetryScheduled | S::Failed)
                && matches!(
                    provisioning_stage,
                    Stage::BootstrapInitialTurn | Stage::FinalizePublication
                ))
            || (!matches!(
                creation_status,
                S::Published | S::DeliveryPending | S::DeliveryFailed | S::DeletionPending
            ) && !matches!(
                initial_execution,
                NativeCreateInitialExecution::Unavailable {}
            ))
        {
            return Err(native_protocol(
                "publication/initial execution does not match creation state",
            ));
        }
        if let NativeCreatePublication::Available {
            canonical_route, ..
        } = publication
        {
            // Slugs are destination-owned, but the route must remain a single canonical product route.
            if !native_product_route_shape(canonical_route) {
                return Err(native_protocol("published route is not canonical"));
            }
        }
    }
    Ok(())
}

fn native_verify_status_stage(
    updated_at_unix_micros: i64,
    initial_execution: &NativeCreateInitialExecution,
) -> Result<(), FederationClientError> {
    if updated_at_unix_micros < 0
        || chrono::DateTime::from_timestamp_micros(updated_at_unix_micros).is_none()
    {
        return Err(native_protocol("creation stage timestamp is invalid"));
    }
    if matches!(initial_execution, NativeCreateInitialExecution::Failed { reason } if reason.trim().is_empty())
    {
        return Err(native_protocol(
            "failed initial execution has no failure reason",
        ));
    }
    Ok(())
}

fn native_product_route_shape(route: &str) -> bool {
    let Some(segment) = route.strip_prefix("/c/") else {
        return false;
    };
    if segment.is_empty() || segment.contains('/') || segment.contains('?') || segment.contains('#')
    {
        return false;
    }
    let Ok(decoded) = percent_encoding::percent_decode_str(segment).decode_utf8() else {
        return false;
    };
    native_option_string(&decoded)
        && !decoded.contains('/')
        && !decoded.contains('?')
        && !decoded.contains('#')
        && crate::api::product_conversations::product_route(Some(&decoded), "") == route
}

async fn remote_native_create_status_post(
    peer: &ResolvedPeer,
    request: &NativeCreateStatusRequest,
) -> Result<NativeCreateStatusResponse, FederationClientError> {
    let response = native_post(
        peer,
        "/api/federation/peer/native-create-status",
        request,
        NATIVE_CREATE_STATUS_REQUEST_BYTES,
        NATIVE_CREATE_STATUS_RESPONSE_BYTES,
    )
    .await?;
    native_verify_status(request, &response)?;
    Ok(response)
}

pub(crate) async fn remote_native_create_status(
    db: &Database,
    peer: &ResolvedPeer,
    request: &NativeCreateStatusRequest,
) -> Result<NativeCreateStatusResponse, FederationClientError> {
    native_client_identity(
        db,
        peer,
        request.caller_instance_id,
        request.destination_instance_id,
    )
    .await?;
    remote_native_create_status_post(peer, request).await
}

pub(crate) async fn remote_native_create(
    db: &Database,
    peer: &ResolvedPeer,
    request: &NativeCreateRequest,
) -> Result<NativeCreateOutcome, FederationClientError> {
    native_client_identity(
        db,
        peer,
        request.caller_instance_id,
        request.destination_instance_id,
    )
    .await?;
    if !native_create_intent_valid(request) {
        return Err(native_protocol("invalid create intent"));
    }
    let post = native_prepare_post(
        peer,
        "/api/federation/peer/native-create",
        request,
        NATIVE_CREATE_REQUEST_BYTES,
    )?;
    let result = native_send_post::<NativeCreateResponse>(post, NATIVE_CREATE_RESPONSE_BYTES)
        .await
        .and_then(|acceptance| {
            native_echo_matches(
                request.caller_instance_id,
                request.destination_instance_id,
                acceptance.caller_instance_id,
                acceptance.destination_instance_id,
                &request.request_id,
                &acceptance.request_id,
            )?;
            if !native_domain_identity_valid(acceptance.product_conversation_id.as_str()) {
                return Err(native_protocol(
                    "accepted create product conversation identity is invalid",
                ));
            }
            Ok(acceptance)
        });
    match result {
        Ok(acceptance) => Ok(NativeCreateOutcome::Accepted { acceptance }),
        Err(error)
            if matches!(
                &error,
                FederationClientError::Request(_) | FederationClientError::NativeRemoteFailure(_)
            ) =>
        {
            let status_request = NativeCreateStatusRequest {
                caller_instance_id: request.caller_instance_id,
                destination_instance_id: request.destination_instance_id,
                request: request.request_id.clone(),
            };
            if let Err(status_error) = native_client_identity(
                db,
                peer,
                status_request.caller_instance_id,
                status_request.destination_instance_id,
            )
            .await
            {
                return Err(FederationClientError::NativeCreateUncertain {
                    destination_instance_id: request.destination_instance_id,
                    request_id: request.request_id.clone(),
                    message: format!(
                        "{error}; original-destination status lookup failed: {status_error}"
                    ),
                });
            }
            if let Err(authority_error) = db
                .require_current_coordinator_transcript(&request.source.source_transcript_id)
                .await
            {
                return Err(FederationClientError::NativeCreateUncertain {
                    destination_instance_id: request.destination_instance_id,
                    request_id: request.request_id.clone(),
                    message: format!("{error}; status lookup not sent: {authority_error}"),
                });
            }
            match remote_native_create_status_post(peer, &status_request).await {
                Ok(status) => Ok(NativeCreateOutcome::Reconciled { status }),
                Err(status_error) => Err(FederationClientError::NativeCreateUncertain {
                    destination_instance_id: request.destination_instance_id,
                    request_id: request.request_id.clone(),
                    message: format!(
                        "{error}; original-destination status lookup failed: {status_error}"
                    ),
                }),
            }
        }
        Err(error) => Err(error),
    }
}

fn native_verify_rename(
    request: &NativeRenameRequest,
    response: &NativeRenameResponse,
) -> Result<(), FederationClientError> {
    let (caller, destination, operation, product, root) = match response {
        NativeRenameResponse::Applied {
            caller_instance_id,
            destination_instance_id,
            operation_id,
            product_conversation_id,
            root_transcript_id,
            ..
        }
        | NativeRenameResponse::Replayed {
            caller_instance_id,
            destination_instance_id,
            operation_id,
            product_conversation_id,
            root_transcript_id,
            ..
        }
        | NativeRenameResponse::Stale {
            caller_instance_id,
            destination_instance_id,
            operation_id,
            product_conversation_id,
            root_transcript_id,
        }
        | NativeRenameResponse::Conflict {
            caller_instance_id,
            destination_instance_id,
            operation_id,
            product_conversation_id,
            root_transcript_id,
        }
        | NativeRenameResponse::NotFound {
            caller_instance_id,
            destination_instance_id,
            operation_id,
            product_conversation_id,
            root_transcript_id,
        }
        | NativeRenameResponse::Unavailable {
            caller_instance_id,
            destination_instance_id,
            operation_id,
            product_conversation_id,
            root_transcript_id,
        } => (
            *caller_instance_id,
            *destination_instance_id,
            operation_id,
            product_conversation_id,
            root_transcript_id,
        ),
    };
    native_echo_matches(
        request.caller_instance_id,
        request.destination_instance_id,
        caller,
        destination,
        &request.operation_id,
        operation,
    )?;
    if product != &request.product_conversation_id || root != &request.expected_root_transcript_id {
        return Err(native_protocol(
            "rename target witnesses do not match request",
        ));
    }
    native_verify_rename_receipt(request, response, product)
}

fn native_verify_rename_receipt(
    request: &NativeRenameRequest,
    response: &NativeRenameResponse,
    product: &ProductConversationId,
) -> Result<(), FederationClientError> {
    match response {
        NativeRenameResponse::Applied {
            receipt_title,
            receipt_slug_at_commit,
            receipt_name_revision_before,
            receipt_committed_at_unix_micros,
            receipt_source_instance_id,
            receipt_source,
            ..
        }
        | NativeRenameResponse::Replayed {
            receipt_title,
            receipt_slug_at_commit,
            receipt_name_revision_before,
            receipt_committed_at_unix_micros,
            receipt_source_instance_id,
            receipt_source,
            ..
        } => {
            let title =
                crate::conversation_actions::normalize_product_conversation_title(&request.title)
                    .map_err(|_| native_protocol("invalid rename title"))?;
            let source_matches = if matches!(response, NativeRenameResponse::Applied { .. }) {
                receipt_source == &request.source
            } else {
                receipt_source.product_conversation == request.source.product_conversation
                    && receipt_source.source_transcript_id == request.source.source_transcript_id
            };
            if receipt_source_instance_id != &request.caller_instance_id
                || !native_source_valid(receipt_source)
                || !source_matches
                || receipt_title != &title
                || receipt_name_revision_before != &request.expected_name_revision
                || *receipt_name_revision_before < 0
                || *receipt_name_revision_before == i64::MAX
                || *receipt_committed_at_unix_micros < 0
                || chrono::DateTime::from_timestamp_micros(*receipt_committed_at_unix_micros)
                    .is_none()
                || !native_product_route_shape(&crate::api::product_conversations::product_route(
                    Some(receipt_slug_at_commit),
                    product.as_str(),
                ))
            {
                return Err(native_protocol(
                    "rename receipt does not witness the requested mutation",
                ));
            }
        }
        NativeRenameResponse::Stale { .. }
        | NativeRenameResponse::Conflict { .. }
        | NativeRenameResponse::NotFound { .. }
        | NativeRenameResponse::Unavailable { .. } => {}
    }
    Ok(())
}

pub(crate) async fn remote_native_rename(
    db: &Database,
    peer: &ResolvedPeer,
    request: &NativeRenameRequest,
) -> Result<NativeRenameResponse, FederationClientError> {
    native_client_identity(
        db,
        peer,
        request.caller_instance_id,
        request.destination_instance_id,
    )
    .await?;
    if !native_rename_intent_valid(request) {
        return Err(native_protocol("invalid rename intent"));
    }
    let response = native_post(
        peer,
        "/api/federation/peer/native-rename",
        request,
        NATIVE_RENAME_REQUEST_BYTES,
        NATIVE_RENAME_RESPONSE_BYTES,
    )
    .await?;
    native_verify_rename(request, &response)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod goal_read;
    #[path = "../native_actions.rs"]
    mod native_actions;
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

    async fn save_test_peer(
        state: &AppState,
        id: InstanceId,
        name: &str,
        url: &str,
        credential_byte: &str,
    ) {
        let credential = phoenix_core::domain::instance_identity::PeerBearerCredential::parse(
            format!("phx_peer_{}", credential_byte.repeat(43)),
        )
        .unwrap();
        state
            .db
            .save_federation_peer_connection(
                id,
                &name.parse().unwrap(),
                &url.parse().unwrap(),
                &credential,
                &PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();
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

    #[tokio::test]
    async fn watch_uuid_selector_does_not_switch_to_another_peers_uuid_shaped_name() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let absent_id = InstanceId::new();
        let other_id = InstanceId::new();
        save_test_peer(
            &state,
            other_id,
            &absent_id.to_string(),
            "https://other.example",
            "a",
        )
        .await;
        let crate::coordinator_tools::instance::InstanceSelector::Remote(selector) =
            serde_json::from_value(serde_json::json!(absent_id.to_string())).unwrap()
        else {
            panic!("UUID selector must be remote");
        };
        assert!(matches!(
            resolve_tool_peer(
                &state.db,
                &selector,
                crate::coordinator_tools::instance::PeerResolution::WatchUuidIdentity,
            )
            .await,
            Err(FederationClientError::PeerNotFound)
        ));
        let peer = resolve_tool_peer(
            &state.db,
            &selector,
            crate::coordinator_tools::instance::PeerResolution::SavedUuidOrName,
        )
        .await
        .unwrap();
        assert_eq!(peer.instance_id(), other_id);
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
    async fn saved_peer_selection_uses_exact_uuid_or_unique_name_and_refuses_ambiguity() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let first = InstanceId::from_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
        let second = InstanceId::new();
        let shadow = InstanceId::new();
        let first_id = first.to_string();
        let uppercase_first_id = first_id.to_uppercase();
        for (id, name, url, credential_byte) in [
            (first, "unique", "https://first.example", "a"),
            (second, "duplicate", "https://second.example", "b"),
            (shadow, first_id.as_str(), "https://shadow.example", "c"),
        ] {
            save_test_peer(&state, id, name, url, credential_byte).await;
        }
        let uppercase_uuid_alias = InstanceId::new();
        save_test_peer(
            &state,
            uppercase_uuid_alias,
            &uppercase_first_id,
            "https://uppercase-uuid-name.example",
            "f",
        )
        .await;
        let uuid_shaped_name = InstanceId::new().to_string();
        let uuid_named_peer = InstanceId::new();
        save_test_peer(
            &state,
            uuid_named_peer,
            &uuid_shaped_name,
            "https://uuid-name.example",
            "e",
        )
        .await;

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
        save_test_peer(
            &state,
            InstanceId::new(),
            "duplicate",
            "https://other.example",
            "d",
        )
        .await;
        assert!(matches!(
            resolve_saved_peer_id(&state.db, "duplicate").await,
            Err(FederationClientError::PeerAmbiguous)
        ));
        assert!(matches!(
            resolve_saved_peer_id(&state.db, "missing").await,
            Err(FederationClientError::PeerNotFound)
        ));
        let local = state.db.instance_id().await.unwrap();
        save_test_peer(&state, local, "self", "https://self.example", "9").await;
        assert!(matches!(
            automatic_continuation_peer(&state.db, "self").await,
            Err(FederationClientError::SelfPeerSelector)
        ));
        for selector in [" unique", "unique ", " unique ", "Unique", "UNIQUE"] {
            assert!(matches!(
                resolve_saved_peer_id(&state.db, selector).await,
                Err(FederationClientError::PeerNotFound)
            ));
        }
        for selector in [format!(" {first} "), first.to_string().replace('-', "")] {
            assert!(matches!(
                resolve_saved_peer_id(&state.db, &selector).await,
                Err(FederationClientError::PeerNotFound)
            ));
        }
        assert_eq!(
            resolve_saved_peer_id(&state.db, &uppercase_first_id)
                .await
                .unwrap(),
            uppercase_uuid_alias
        );
    }

    #[test]
    fn remote_read_rejects_oversized_cursor_before_peer_lookup() {
        let error = validate_remote_read_cursor(
            super::super::global_read::MAX_CONVERSATION_READ_CURSOR + 1,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            FederationClientError::RemoteRejected(message)
                if message.contains("cursor exceeds maximum")
        ));
    }

    #[test]
    fn remote_read_cursor_target_guard_rejects_product_continuations() {
        let target = RemoteConversationTarget::parse("@conv:missing-product").unwrap();
        assert_eq!(
            validate_remote_cursor_target(&target, 1),
            Err(REMOTE_STABLE_CONTINUATION_ERROR)
        );
        assert_eq!(validate_remote_cursor_target(&target, 0), Ok(()));

        let exact_target =
            RemoteConversationTarget::parse("@transcript:missing-transcript").unwrap();
        assert_eq!(validate_remote_cursor_target(&exact_target, 1), Ok(()));
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
                &"caller".parse().unwrap(),
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

    fn automatic_continuation_response(
        destination_instance_id: InstanceId,
        caller_instance_id: InstanceId,
        aggregate: crate::api::AutomaticContinuationAggregateView,
    ) -> RemoteAutomaticContinuationResponse {
        RemoteAutomaticContinuationResponse {
            destination_instance_id,
            caller_instance_id,
            preference: crate::automatic_continuation_settings::native::PreferenceView {
                aggregate,
                auto_continue_on_context_exhaustion: true,
            },
        }
    }

    #[test]
    fn automatic_continuation_wire_shapes_and_response_provenance() {
        let destination = InstanceId::new();
        let caller = InstanceId::new();
        let target = RemoteAutomaticContinuationTarget::Coordinator {};
        let base = serde_json::json!({
            "destination_instance_id": destination,
            "caller_instance_id": caller,
            "target": {"kind":"coordinator"}
        });
        assert!(
            serde_json::from_value::<RemoteAutomaticContinuationReadRequest>(base.clone()).is_ok()
        );
        assert!(
            serde_json::from_value::<RemoteAutomaticContinuationSetRequest>(base.clone()).is_err()
        );
        for value in [serde_json::json!(true), serde_json::json!(false)] {
            let mut set = base.clone();
            set["auto_continue_on_context_exhaustion"] = value;
            let request: RemoteAutomaticContinuationSetRequest =
                serde_json::from_value(set.clone()).unwrap();
            assert_eq!(serde_json::to_value(&request).unwrap(), set);
            assert!(serde_json::from_value::<RemoteAutomaticContinuationReadRequest>(set).is_err());
        }
        for value in [
            serde_json::Value::Null,
            serde_json::json!("true"),
            serde_json::json!(1),
        ] {
            let mut set = base.clone();
            set["auto_continue_on_context_exhaustion"] = value;
            assert!(serde_json::from_value::<RemoteAutomaticContinuationSetRequest>(set).is_err());
        }
        assert!(matches!(
            verify_automatic_continuation_response(
                automatic_continuation_response(
                    InstanceId::new(),
                    caller,
                    crate::api::AutomaticContinuationAggregateView::Coordinator {
                        product_conversation_id: "coordinator".into()
                    }
                ),
                destination,
                caller,
                &target
            ),
            Err(FederationClientError::DestinationMismatch)
        ));
        assert!(matches!(
            verify_automatic_continuation_response(
                automatic_continuation_response(
                    destination,
                    InstanceId::new(),
                    crate::api::AutomaticContinuationAggregateView::Coordinator {
                        product_conversation_id: "coordinator".into()
                    }
                ),
                destination,
                caller,
                &target
            ),
            Err(FederationClientError::CallerMismatch)
        ));
        assert!(matches!(
            verify_automatic_continuation_response(
                automatic_continuation_response(
                    destination,
                    caller,
                    crate::api::AutomaticContinuationAggregateView::Ordinary {
                        product_conversation_id: "other".into()
                    }
                ),
                destination,
                caller,
                &target
            ),
            Err(FederationClientError::ProvenanceMismatch(_))
        ));
        assert!(matches!(
            verify_automatic_continuation_response(
                automatic_continuation_response(
                    destination,
                    caller,
                    crate::api::AutomaticContinuationAggregateView::Coordinator {
                        product_conversation_id: "   ".into()
                    }
                ),
                destination,
                caller,
                &target
            ),
            Err(FederationClientError::ProvenanceMismatch(_))
        ));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn peer_automatic_continuation_roundtrip_rejects_bad_identity_and_revocation() {
        use tower::ServiceExt as _;
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let ordinary = state
            .db
            .create_conversation("peer-settings", "peer-settings", "/tmp", true, None, None)
            .await
            .unwrap();
        let destination = state.db.instance_id().await.unwrap();
        let caller = InstanceId::new();
        let bearer = format!("phx_peer_{}", "b".repeat(43));
        state
            .db
            .replace_federation_enrollment(
                caller,
                &"peer-settings-caller".parse().unwrap(),
                &FederationCredentialVerifier::from_bearer(bearer.as_bytes()),
            )
            .await
            .unwrap();
        let app = crate::api::handlers::create_router(state.clone());
        let target = serde_json::json!({"kind":"ordinary", "product_conversation_id":ordinary.product_conversation_id});
        let send = |path: &str, body: serde_json::Value, auth: Option<&str>| {
            let mut request = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json");
            if let Some(token) = auth {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            request
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        };
        let read_path = "/api/federation/peer/read-automatic-continuation";
        let set_path = "/api/federation/peer/set-automatic-continuation";
        let body = serde_json::json!({"destination_instance_id":destination, "caller_instance_id":caller, "target":target});
        assert_eq!(
            app.clone()
                .oneshot(send(read_path, body.clone(), None))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let read = app
            .clone()
            .oneshot(send(read_path, body.clone(), Some(&bearer)))
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(read.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: RemoteAutomaticContinuationResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response.destination_instance_id, destination);
        assert_eq!(response.caller_instance_id, caller);
        assert!(!response.preference.auto_continue_on_context_exhaustion);
        assert_eq!(
            response.preference.aggregate,
            crate::api::AutomaticContinuationAggregateView::Ordinary {
                product_conversation_id: ordinary.product_conversation_id.to_string(),
            }
        );
        let mut read_with_bool = body.clone();
        read_with_bool["auto_continue_on_context_exhaustion"] = serde_json::json!(false);
        assert_eq!(
            app.clone()
                .oneshot(send(read_path, read_with_bool, Some(&bearer)))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            app.clone()
                .oneshot(send(set_path, body.clone(), Some(&bearer)))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let mut null_set = body.clone();
        null_set["auto_continue_on_context_exhaustion"] = serde_json::Value::Null;
        assert_eq!(
            app.clone()
                .oneshot(send(set_path, null_set, Some(&bearer)))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let mut setting = body.clone();
        setting["auto_continue_on_context_exhaustion"] = serde_json::json!(true);
        let mut wrong = setting.clone();
        wrong["destination_instance_id"] = serde_json::json!(InstanceId::new());
        assert_eq!(
            app.clone()
                .oneshot(send(set_path, wrong, Some(&bearer)))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        let mut wrong = setting.clone();
        wrong["caller_instance_id"] = serde_json::json!(InstanceId::new());
        assert_eq!(
            app.clone()
                .oneshot(send(set_path, wrong, Some(&bearer)))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(send(set_path, setting.clone(), Some(&bearer)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let read = app
            .clone()
            .oneshot(send(read_path, body.clone(), Some(&bearer)))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(read.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: RemoteAutomaticContinuationResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(response.preference.auto_continue_on_context_exhaustion);
        assert!(state
            .db
            .automatic_continuation_admission(&ordinary.id)
            .await
            .unwrap()
            .is_none());
        state.db.revoke_federation_enrollment(caller).await.unwrap();
        assert_ne!(
            app.clone()
                .oneshot(send(read_path, body, Some(&bearer)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_ne!(
            app.oneshot(send(set_path, setting, Some(&bearer)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
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
                &"caller".parse().unwrap(),
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
    async fn reserved_local_peer_names_reject_enrollment_and_import_without_replacement() {
        use axum::{body::Body, http::Request};
        use phoenix_core::domain::instance_identity::{PeerBaseUrl, PeerBearerCredential};
        use tower::ServiceExt as _;

        let mut state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        state.password = Some(
            phoenix_core::domain::instance_identity::OwnerPassword::parse("owner-password".into())
                .unwrap(),
        );
        let local = state.db.instance_id().await.unwrap();
        let peer = InstanceId::new();
        let verifier = FederationCredentialVerifier::from_bearer(b"unchanged-enrollment");
        state
            .db
            .replace_federation_enrollment(peer, &"remote-peer".parse().unwrap(), &verifier)
            .await
            .unwrap();
        let bearer = PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap();
        let url: PeerBaseUrl = "https://peer.example".parse().unwrap();
        state
            .db
            .save_federation_peer_connection(
                peer,
                &"remote-peer".parse().unwrap(),
                &url,
                &bearer,
                &PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();
        let app = crate::api::handlers::create_router(state.clone());
        for name in ["local", "LOCAL", " \tLoCaL\n"] {
            for (path, body) in [
                (
                    "/api/federation/enrollments",
                    serde_json::json!({"caller_instance_id":peer,"caller_display_name":name}),
                ),
                (
                    "/api/federation/peers/import",
                    serde_json::json!({
                        "peer_display_name":name,"base_url":"https://replacement.example",
                        "enrollment":{"receiver_instance_id":peer,"caller_instance_id":local,"token":bearer.expose(),"tls_trust":PeerTlsTrust::PlatformRoots}
                    }),
                ),
            ] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(path)
                            .header("content-type", "application/json")
                            .header("authorization", "Bearer owner-password")
                            .body(Body::from(body.to_string()))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let body = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                assert!(std::str::from_utf8(&body).unwrap().contains("reserved"));
            }
        }
        let active = state
            .db
            .authenticate_federation_verifier(&verifier)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.caller_display_name, "remote-peer");
        let saved = state
            .db
            .federation_peer_connection(peer)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.peer_display_name, "remote-peer");
        assert_eq!(saved.base_url, url);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM federation_enrollments")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn peer_import_rejects_bundle_for_another_caller() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let response = import_peer_connection(
            OwnerAuthenticated,
            State(state),
            Json(ImportPeerConnectionRequest {
                peer_display_name: "peer".parse().unwrap(),
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
                &"remote".parse().unwrap(),
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

        let peer = resolve_saved_peer_for_test(&state.db, "remote")
            .await
            .unwrap();
        resolve_remote_reference(&state.db, &peer, "@conv:product")
            .await
            .unwrap();
        assert!(matches!(
            resolve_saved_peer_for_test(&state.db, "missing").await,
            Err(FederationClientError::PeerNotFound)
        ));
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
        use phoenix_core::domain::instance_identity::PeerBearerCredential;
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let peer = InstanceId::new();
        let token = PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap();

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
                    &"peer".parse().unwrap(),
                    &format!("https://localhost:{}", source_address.port())
                        .parse()
                        .unwrap(),
                    &token,
                    &trust,
                )
                .await
                .unwrap();

            let resolved_peer = resolve_saved_peer_for_test(&state.db, "peer")
                .await
                .unwrap();
            let result = query_remote_database(&state.db, &resolved_peer, "SELECT 1").await;
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

    #[tokio::test]
    async fn automatic_continuation_identity_refusals_remain_pending_after_authority_closes() {
        for mismatch in [StatusCode::CONFLICT, StatusCode::UNAUTHORIZED] {
            let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
            let runtime = state.runtime.clone();
            let destination = state.db.instance_id().await.unwrap();
            let caller = InstanceId::new();
            let request = RemoteAutomaticContinuationSetRequest {
                destination_instance_id: if mismatch == StatusCode::CONFLICT {
                    InstanceId::new()
                } else {
                    destination
                },
                caller_instance_id: if mismatch == StatusCode::UNAUTHORIZED {
                    InstanceId::new()
                } else {
                    caller
                },
                target: RemoteAutomaticContinuationTarget::Coordinator {},
                auto_continue_on_context_exhaustion: true,
            };
            let peer = PeerAuthenticated {
                send_authority: phoenix_db::FederationSendAuthority::new(
                    caller,
                    phoenix_core::domain::instance_identity::FederationCredentialVerifier::from_bearer(
                        b"peer-test",
                    ),
                ),
                caller_instance_id: caller,
                caller_display_name: "peer".to_string(),
            };
            let owner = runtime.acquire_local_authority_pass().unwrap();
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let identity = async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(destination)
            };
            let verification =
                verify_set_automatic_continuation_identity(&runtime, &peer, &request, identity);
            tokio::pin!(verification);
            tokio::select! {
                result = &mut verification => panic!("identity verification finished before lookup release: {result:?}"),
                started = started_rx => started.unwrap(),
            }

            owner.close("test_identity_lookup_authority_closure");
            release_tx.send(()).unwrap();
            assert!(tokio::select! {
                biased;
                _ = &mut verification => false,
                () = std::future::ready(()) => true,
            });
        }
    }

    async fn assert_response_remains_pending_after_authority_closes(
        runtime: &crate::runtime::RuntimeManager,
        response: Response,
    ) {
        let owner = runtime.acquire_local_authority_pass().unwrap();
        owner.close("test_native_create_response_authority_closure");
        let response = native_create_response(runtime, response);
        tokio::pin!(response);
        assert!(tokio::select! {
            biased;
            _ = &mut response => false,
            () = std::future::ready(()) => true,
        });
    }

    fn test_native_create_peer(caller: InstanceId) -> PeerAuthenticated {
        PeerAuthenticated {
            send_authority: phoenix_db::FederationSendAuthority::new(
                caller,
                phoenix_core::domain::instance_identity::FederationCredentialVerifier::from_bearer(
                    b"peer-test",
                ),
            ),
            caller_instance_id: caller,
            caller_display_name: "peer".to_string(),
        }
    }

    fn test_native_create_request(
        caller: InstanceId,
        destination: InstanceId,
        cwd: String,
    ) -> NativeCreateRequest {
        NativeCreateRequest {
            caller_instance_id: caller,
            destination_instance_id: destination,
            request_id: uuid::Uuid::new_v4().to_string(),
            cwd,
            objective: "test".to_string(),
            model: None,
            effort: None,
            language: None,
            start: NativeCreateStart::SubmitObjective,
            source: NativeActionSourceCall {
                product_conversation: ProductConversationId::parse("source").unwrap(),
                source_transcript_id: "transcript".to_string(),
                source_assistant_message_id: "message".to_string(),
                source_tool_use_id: "tool".to_string(),
            },
        }
    }

    #[tokio::test]
    async fn native_create_error_response_remains_pending_after_authority_closes() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let caller = InstanceId::new();
        let peer = test_native_create_peer(caller);
        let request = test_native_create_request(caller, InstanceId::new(), "/tmp".to_string());
        let response = native_create_inner(state.clone(), peer, Ok(Json(request)))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        assert_response_remains_pending_after_authority_closes(&state.runtime, response).await;
    }

    #[tokio::test]
    async fn native_create_success_response_remains_pending_after_authority_closes() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let caller = InstanceId::new();
        let local = state.db.instance_id().await.unwrap();
        let cwd = state.runtime.runtime_home().join("native-create-cwd");
        std::fs::create_dir(&cwd).unwrap();
        let peer = test_native_create_peer(caller);
        let request = test_native_create_request(caller, local, cwd.to_string_lossy().into_owned());
        let response = native_create_inner(state.clone(), peer, Ok(Json(request)))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        assert_response_remains_pending_after_authority_closes(&state.runtime, response).await;
    }

    #[tokio::test]
    async fn peer_authority_closure_and_write_panic_remain_pending() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let runtime = state.runtime.clone();
        let owner = runtime.acquire_local_authority_pass().unwrap();
        let pass = crate::automatic_continuation_settings::native::PreferenceWritePass::new(owner);
        assert!(pass
            .classify::<()>(phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified)
            .is_err());
        assert!(runtime.local_authority_is_closed());
        let closed_response = peer_authority_response(&runtime, StatusCode::OK.into_response());
        tokio::pin!(closed_response);
        assert!(tokio::select! {
            biased;
            _ = &mut closed_response => false,
            () = std::future::ready(()) => true,
        });

        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let runtime = state.runtime.clone();
        let worker_runtime = runtime.clone();
        let join = tokio::spawn(async move {
            let owner = worker_runtime.acquire_local_authority_pass().unwrap();
            let _pass =
                crate::automatic_continuation_settings::native::PreferenceWritePass::new(owner);
            panic!("injected write worker panic");
            #[allow(unreachable_code)]
            StatusCode::OK.into_response()
        });
        let result = join.await;
        assert!(result.is_err());
        assert!(runtime.local_authority_is_closed());
        let response = peer_write_join_response(&runtime, result);
        tokio::pin!(response);
        assert!(tokio::select! {
            biased;
            _ = &mut response => false,
            () = std::future::ready(()) => true,
        });
    }
}
