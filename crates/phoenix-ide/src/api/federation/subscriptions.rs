use super::{append_bounded_response_chunk, remote_query_client, FederationClientError};
use crate::{
    api::{
        auth::{OwnerAuthenticated, PeerAuthenticated},
        AppState,
    },
    runtime::RuntimeManager,
};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use futures::StreamExt;
use phoenix_core::domain::{
    federation_subscription::{
        SubscriptionAuthorization, SubscriptionCursor, SubscriptionDelivery, SubscriptionReference,
        SubscriptionReplay, SubscriptionSnapshot, SubscriptionState, REPLAY_BATCH,
    },
    instance_identity::{InstanceId, InstanceSelector},
    product_conversation::ProductConversationId,
};
use phoenix_db::federation_subscriptions::RemoteNotificationDisposition;
use phoenix_db::{
    federation_subscriptions::RemoteSubscription, Database, FederationPeerConnection,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

pub const REQUEST_BYTES: usize = 8 * 1024;
pub(crate) const RESPONSE_BYTES: usize = 64 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum SubscriptionOperation {
    Create,
    Replay { after: SubscriptionCursor },
    Authorize { delivery: SubscriptionDelivery },
    Unsubscribe,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct SubscriptionRequest {
    pub caller_instance_id: InstanceId,
    pub reference: SubscriptionReference,
    pub subscription_id: Uuid,
    pub operation: SubscriptionOperation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum SubscriptionResult {
    Created {
        snapshot: SubscriptionSnapshot,
    },
    Replay {
        replay: SubscriptionReplay,
    },
    Authorization {
        authorization: SubscriptionAuthorization,
        navigation: Option<SubscriptionNavigation>,
    },
    Unsubscribed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct SubscriptionResponse {
    pub caller_instance_id: InstanceId,
    pub reference: SubscriptionReference,
    pub subscription_id: Uuid,
    pub result: SubscriptionResult,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct SubscriptionNavigation {
    pub stable_path: String,
    pub source_path: String,
}

fn rejected(status: StatusCode) -> Response {
    (
        status,
        Json(super::openapi::MessageErrorBody {
            error: "subscription request unavailable or invalid".into(),
        }),
    )
        .into_response()
}
#[allow(clippy::too_many_lines)]
async fn apply(
    peer: PeerAuthenticated,
    state: AppState,
    request: SubscriptionRequest,
    operation: &'static str,
) -> Response {
    let Ok(destination) = state.db.instance_id().await else {
        return rejected(StatusCode::INTERNAL_SERVER_ERROR);
    };
    if request.caller_instance_id != peer.caller_instance_id
        || request.reference.instance_id != destination
        || request.reference.product_conversation_id.as_str().len() > 1024
    {
        return rejected(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let result = match request.operation {
        SubscriptionOperation::Create if operation == "create" => state
            .db
            .create_federation_subscription(
                peer.enrollment_id,
                peer.caller_instance_id,
                request.subscription_id,
                &request.reference.product_conversation_id,
                Utc::now().timestamp_micros(),
            )
            .await
            .map(|snapshot| SubscriptionResult::Created { snapshot }),
        SubscriptionOperation::Replay { after } if operation == "replay" => state
            .db
            .replay_federation_subscription(
                peer.enrollment_id,
                peer.caller_instance_id,
                request.subscription_id,
                &request.reference.product_conversation_id,
                after,
                Utc::now().timestamp_micros(),
            )
            .await
            .map(|replay| SubscriptionResult::Replay { replay }),
        SubscriptionOperation::Authorize { ref delivery } if operation == "authorize" => {
            async {
                let authorization = state
                    .db
                    .authorize_federation_delivery(
                        peer.enrollment_id,
                        peer.caller_instance_id,
                        request.subscription_id,
                        &request.reference.product_conversation_id,
                        delivery,
                    )
                    .await?;
                let navigation =
                    if matches!(authorization, SubscriptionAuthorization::Authorized { .. }) {
                        let transcript = match delivery {
                            SubscriptionDelivery::Fact { fact } => &fact.source_transcript_id,
                            SubscriptionDelivery::Gap { gap } => &gap.snapshot.transcript_id,
                        };
                        let aggregate = state
                            .db
                            .get_ordinary_product_conversation(
                                &request.reference.product_conversation_id,
                            )
                            .await?;
                        Some(SubscriptionNavigation {
                            stable_path: crate::api::product_conversations::canonical_route(
                                &aggregate,
                            ),
                            source_path: crate::api::product_conversations::exact_transcript_route(
                                transcript,
                            ),
                        })
                    } else {
                        None
                    };
                Ok(SubscriptionResult::Authorization {
                    authorization,
                    navigation,
                })
            }
            .await
        }
        SubscriptionOperation::Unsubscribe if operation == "unsubscribe" => state
            .db
            .unsubscribe_federation_subscription(
                peer.enrollment_id,
                peer.caller_instance_id,
                request.subscription_id,
                &request.reference.product_conversation_id,
            )
            .await
            .map(|()| SubscriptionResult::Unsubscribed),
        SubscriptionOperation::Create
        | SubscriptionOperation::Replay { .. }
        | SubscriptionOperation::Authorize { .. }
        | SubscriptionOperation::Unsubscribe => {
            return rejected(StatusCode::UNPROCESSABLE_ENTITY);
        }
    };
    match result {
        Ok(result) => Json(SubscriptionResponse {
            caller_instance_id: peer.caller_instance_id,
            reference: request.reference,
            subscription_id: request.subscription_id,
            result,
        })
        .into_response(),
        Err(error) => {
            tracing::debug!(%error, "federation subscription rejected");
            rejected(StatusCode::CONFLICT)
        }
    }
}
pub async fn create(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<SubscriptionRequest>,
) -> Response {
    apply(peer, state, request, "create").await
}
pub async fn replay(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<SubscriptionRequest>,
) -> Response {
    apply(peer, state, request, "replay").await
}
pub async fn authorize(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<SubscriptionRequest>,
) -> Response {
    apply(peer, state, request, "authorize").await
}
pub async fn unsubscribe(
    peer: PeerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<SubscriptionRequest>,
) -> Response {
    apply(peer, state, request, "unsubscribe").await
}

fn mismatch() -> FederationClientError {
    FederationClientError::RemoteRejected(
        "subscription response identity, shape or order mismatch".into(),
    )
}
fn validate_response(
    request: &SubscriptionRequest,
    response: &SubscriptionResponse,
) -> Result<(), FederationClientError> {
    if response.caller_instance_id != request.caller_instance_id
        || response.reference != request.reference
        || response.subscription_id != request.subscription_id
    {
        return Err(mismatch());
    }
    match (&request.operation, &response.result) {
        (SubscriptionOperation::Create, SubscriptionResult::Created { snapshot })
            if snapshot.transcript_id.as_str().len() <= 1024 =>
        {
            Ok(())
        }
        (
            SubscriptionOperation::Authorize { delivery },
            SubscriptionResult::Authorization {
                authorization,
                navigation,
            },
        ) => {
            delivery.bounds().map_err(|_| mismatch())?;
            match (authorization, navigation) {
                (SubscriptionAuthorization::Authorized { .. }, Some(navigation)) => {
                    let transcript = match delivery {
                        SubscriptionDelivery::Fact { fact } => &fact.source_transcript_id,
                        SubscriptionDelivery::Gap { gap } => &gap.snapshot.transcript_id,
                    };
                    if !crate::api::product_conversations::canonical_product_route_shape(
                        &navigation.stable_path,
                    ) || navigation.source_path
                        != crate::api::product_conversations::exact_transcript_route(transcript)
                    {
                        return Err(mismatch());
                    }
                    Ok(())
                }
                (
                    SubscriptionAuthorization::Rejected { .. } | SubscriptionAuthorization::Refresh,
                    None,
                ) => Ok(()),
                (SubscriptionAuthorization::Authorized { .. }, None)
                | (
                    SubscriptionAuthorization::Rejected { .. } | SubscriptionAuthorization::Refresh,
                    Some(_),
                ) => Err(mismatch()),
            }
        }
        (SubscriptionOperation::Unsubscribe, SubscriptionResult::Unsubscribed) => Ok(()),
        (SubscriptionOperation::Replay { after }, SubscriptionResult::Replay { replay }) => {
            match replay {
                SubscriptionReplay::Ended { .. } => Ok(()),
                SubscriptionReplay::Gap { gap }
                    if gap.requested_after == *after
                        && gap.resume_after > *after
                        && gap.snapshot.transcript_id.as_str().len() <= 1024 =>
                {
                    Ok(())
                }
                SubscriptionReplay::Facts { events } if events.len() <= REPLAY_BATCH => {
                    let mut previous = after.get();
                    let mut seen = std::collections::HashSet::new();
                    for event in events {
                        if previous.checked_add(1) != Some(event.sequence.get())
                            || event.source_transcript_id.as_str().len() > 1024
                            || event.occurred_at_us < 0
                            || !seen.insert(event.event_id)
                        {
                            return Err(mismatch());
                        }
                        previous = event.sequence.get();
                    }
                    Ok(())
                }
                SubscriptionReplay::Facts { .. } | SubscriptionReplay::Gap { .. } => {
                    Err(mismatch())
                }
            }
        }
        _ => Err(mismatch()),
    }
}
pub(crate) async fn request_peer(
    peer: &FederationPeerConnection,
    request: &SubscriptionRequest,
) -> Result<SubscriptionResponse, FederationClientError> {
    request_peer_with_timeout(peer, request, std::time::Duration::from_secs(5)).await
}

pub(crate) async fn request_peer_with_timeout(
    peer: &FederationPeerConnection,
    request: &SubscriptionRequest,
    timeout: std::time::Duration,
) -> Result<SubscriptionResponse, FederationClientError> {
    if peer.peer_instance_id != request.reference.instance_id
        || request.reference.product_conversation_id.as_str().len() > 1024
    {
        return Err(mismatch());
    }
    let suffix = match request.operation {
        SubscriptionOperation::Create => "create",
        SubscriptionOperation::Replay { .. } => "replay",
        SubscriptionOperation::Authorize { .. } => "authorize",
        SubscriptionOperation::Unsubscribe => "unsubscribe",
    };
    let endpoint = peer
        .base_url
        .navigation_url(&format!("/api/federation/peer/subscriptions/{suffix}"))?;
    let response = remote_query_client(&peer.tls_trust)?
        .post(endpoint.as_url().clone())
        .timeout(timeout)
        .bearer_auth(peer.bearer_credential.expose())
        .json(request)
        .send()
        .await?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(FederationClientError::SubscriptionAuthenticationRejected);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        append_bounded_response_chunk(&mut bytes, &chunk?, RESPONSE_BYTES)?;
    }
    if !status.is_success() {
        return Err(FederationClientError::RemoteRejected(format!(
            "subscription HTTP {}",
            status.as_u16()
        )));
    }
    let response: SubscriptionResponse = serde_json::from_slice(&bytes)?;
    validate_response(request, &response)?;
    Ok(response)
}

#[derive(Serialize)]
struct SubscriptionNotificationLiteral<'a> {
    reference: &'a SubscriptionReference,
    subscription_id: Uuid,
    delivery: &'a SubscriptionDelivery,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(crate) struct RemoteWatchReceipt {
    #[serde(flatten)]
    pub subscription: RemoteSubscription,
    pub snapshot: SubscriptionSnapshot,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(crate) struct RemoteUnsubscribeReceipt {
    pub reference: SubscriptionReference,
    pub subscription_id: Uuid,
    pub local_stopped: bool,
    pub remote_confirmed: bool,
}

async fn request_active_peer_subscription(
    db: &Database,
    peer: &FederationPeerConnection,
    request: &SubscriptionRequest,
) -> Result<SubscriptionResponse, String> {
    let response = request_peer(peer, request).await;
    if matches!(
        response,
        Err(FederationClientError::SubscriptionAuthenticationRejected)
    ) {
        db.settle_remote_subscription_revocation(&request.reference, request.subscription_id)
            .await
            .map_err(|error| error.to_string())?;
    }
    response.map_err(|error| error.to_string())
}

pub(crate) async fn create_saved_peer_subscription(
    db: &Database,
    instance: String,
    product: ProductConversationId,
    requested_id: Option<Uuid>,
) -> Result<RemoteWatchReceipt, String> {
    let selector = InstanceSelector::parse(instance).map_err(|error| error.to_string())?;
    let peer = db
        .resolve_federation_peer(&selector)
        .await
        .map_err(|error| error.to_string())?;
    let reference = SubscriptionReference {
        instance_id: peer.peer_instance_id,
        product_conversation_id: product,
    };
    let existing = db
        .remote_subscription_for_management(&reference)
        .await
        .map_err(|error| error.to_string())?;
    if existing
        .as_ref()
        .is_some_and(|subscription| subscription.pending_unsubscribe)
    {
        return Err(
            "remote unsubscribe remains unconfirmed; retry it before enrolling again".into(),
        );
    }
    let id = existing.as_ref().map_or_else(
        || requested_id.unwrap_or_else(Uuid::new_v4),
        |row| row.subscription_id,
    );
    if requested_id.is_some_and(|requested| requested != id) {
        return Err("an active subscription already exists with a different ID".into());
    }
    // Persist the retry identity before crossing the network; a lost create response reuses it.
    db.save_remote_subscription(&reference, id)
        .await
        .map_err(|error| error.to_string())?;
    let request = SubscriptionRequest {
        caller_instance_id: db.instance_id().await.map_err(|error| error.to_string())?,
        reference: reference.clone(),
        subscription_id: id,
        operation: SubscriptionOperation::Create,
    };
    let response = request_active_peer_subscription(db, &peer, &request).await?;
    let SubscriptionResult::Created { snapshot } = response.result else {
        return Err("enrollment snapshot required".into());
    };
    db.mark_remote_subscription_enrolled(&reference, id)
        .await
        .map_err(|error| error.to_string())?;
    let subscription = db
        .remote_subscription_for_product(&reference)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "subscription ended during enrollment".to_string())?;
    Ok(RemoteWatchReceipt {
        subscription,
        snapshot,
    })
}

pub(crate) async fn unsubscribe_saved_peer(
    db: &Database,
    instance: String,
    product: ProductConversationId,
) -> Result<RemoteUnsubscribeReceipt, String> {
    let selector = InstanceSelector::parse(instance).map_err(|error| error.to_string())?;
    let peer = db
        .resolve_federation_peer(&selector)
        .await
        .map_err(|error| error.to_string())?;
    let reference = SubscriptionReference {
        instance_id: peer.peer_instance_id,
        product_conversation_id: product,
    };
    let remote = db
        .remote_subscription_for_management(&reference)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "active or pending remote subscription not found".to_string())?;
    unsubscribe_remote(db, &peer, &remote).await
}

pub(crate) async fn unsubscribe_remote(
    db: &Database,
    peer: &FederationPeerConnection,
    remote: &RemoteSubscription,
) -> Result<RemoteUnsubscribeReceipt, String> {
    db.begin_remote_unsubscribe(&remote.reference, remote.subscription_id)
        .await
        .map_err(|error| error.to_string())?;
    let response = request_peer(
        peer,
        &SubscriptionRequest {
            caller_instance_id: db.instance_id().await.map_err(|error| error.to_string())?,
            reference: remote.reference.clone(),
            subscription_id: remote.subscription_id,
            operation: SubscriptionOperation::Unsubscribe,
        },
    )
    .await;
    let remote_confirmed = match response {
        Ok(_) => {
            db.complete_remote_unsubscribe(&remote.reference, remote.subscription_id)
                .await
                .map_err(|error| error.to_string())?;
            true
        }
        Err(FederationClientError::SubscriptionAuthenticationRejected) => {
            db.complete_remote_unsubscribe(&remote.reference, remote.subscription_id)
                .await
                .map_err(|error| error.to_string())?;
            false
        }
        Err(error) => {
            tracing::debug!(%error, "remote unsubscribe unconfirmed; durable retry retained and local delivery stopped");
            false
        }
    };
    Ok(RemoteUnsubscribeReceipt {
        reference: remote.reference.clone(),
        subscription_id: remote.subscription_id,
        local_stopped: true,
        remote_confirmed,
    })
}

pub(crate) async fn sync_once(
    db: &Database,
    runtime: &Arc<RuntimeManager>,
    remote: &RemoteSubscription,
) -> Result<(), String> {
    db.touch_remote_subscription(remote)
        .await
        .map_err(|error| error.to_string())?;
    let peer = db
        .federation_peer_connection(remote.reference.instance_id)
        .await
        .map_err(|error| error.to_string())?;
    let remote = db
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "remote subscription unavailable".to_string())?;
    if remote.pending_unsubscribe {
        let peer = peer.ok_or_else(|| "saved peer unavailable; no local fallback".to_string())?;
        unsubscribe_remote(db, &peer, &remote).await?;
        return Ok(());
    }
    if remote.state != SubscriptionState::Active {
        return Ok(());
    }
    drain_notifications(db, runtime, &remote, peer.as_ref()).await?;
    let remote = db
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "remote subscription unavailable".to_string())?;
    if remote.pending_unsubscribe || remote.state != SubscriptionState::Active {
        return Ok(());
    }
    let peer = peer.ok_or_else(|| "saved peer unavailable; no local fallback".to_string())?;
    let caller = db.instance_id().await.map_err(|error| error.to_string())?;
    if !remote.enrolled {
        request_active_peer_subscription(
            db,
            &peer,
            &SubscriptionRequest {
                caller_instance_id: caller,
                reference: remote.reference.clone(),
                subscription_id: remote.subscription_id,
                operation: SubscriptionOperation::Create,
            },
        )
        .await?;
        db.mark_remote_subscription_enrolled(&remote.reference, remote.subscription_id)
            .await
            .map_err(|error| error.to_string())?;
    }
    let response = request_active_peer_subscription(
        db,
        &peer,
        &SubscriptionRequest {
            caller_instance_id: caller,
            reference: remote.reference.clone(),
            subscription_id: remote.subscription_id,
            operation: SubscriptionOperation::Replay {
                after: remote.admitted_cursor,
            },
        },
    )
    .await?;
    let SubscriptionResult::Replay { replay } = response.result else {
        return Err("replay response required".into());
    };
    db.stage_remote_replay(&remote, &replay)
        .await
        .map_err(|error| error.to_string())?;
    drain_notifications(db, runtime, &remote, Some(&peer)).await
}

#[allow(clippy::too_many_lines)]
async fn drain_notifications(
    db: &Database,
    runtime: &Arc<RuntimeManager>,
    remote: &RemoteSubscription,
    peer: Option<&FederationPeerConnection>,
) -> Result<(), String> {
    let Some(target) = db
        .coordinator_watch_target()
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let service =
        crate::send_chat_service::SendChatApplicationService::new(db.clone(), runtime.clone());
    let mut admitted_cursor = db
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "remote subscription unavailable".to_string())?
        .admitted_cursor;
    for notification in db
        .pending_remote_notifications(remote)
        .await
        .map_err(|error| error.to_string())?
    {
        let (after, through) = notification.content.bounds().map_err(str::to_string)?;
        if after != admitted_cursor {
            return Ok(());
        }
        if !matches!(
            notification.disposition,
            RemoteNotificationDisposition::Authorized { .. }
        ) {
            db.request_remote_notification_authorization(remote, &notification.admission_id)
                .await
                .map_err(|error| error.to_string())?;
            let peer = peer
                .ok_or_else(|| "saved peer unavailable for delivery authorization".to_string())?;
            let response = request_peer(
                peer,
                &SubscriptionRequest {
                    caller_instance_id: db
                        .instance_id()
                        .await
                        .map_err(|error| error.to_string())?,
                    reference: remote.reference.clone(),
                    subscription_id: remote.subscription_id,
                    operation: SubscriptionOperation::Authorize {
                        delivery: notification.content.clone(),
                    },
                },
            )
            .await;
            let (authorization, navigation) = match response {
                Ok(SubscriptionResponse {
                    result:
                        SubscriptionResult::Authorization {
                            authorization,
                            navigation,
                        },
                    ..
                }) => (authorization, navigation),
                Err(FederationClientError::SubscriptionAuthenticationRejected) => {
                    db.settle_remote_subscription_revocation(
                        &remote.reference,
                        remote.subscription_id,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                    return Ok(());
                }
                Err(error) => return Err(error.to_string()),
                _ => return Err("authorization receipt required".into()),
            };
            match authorization {
                SubscriptionAuthorization::Authorized { authorization_id } => {
                    let navigation = navigation
                        .ok_or_else(|| "authorized source navigation required".to_string())?;
                    db.confirm_remote_authorization(
                        remote,
                        &notification.admission_id,
                        authorization_id,
                        &navigation.stable_path,
                        &navigation.source_path,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                }
                SubscriptionAuthorization::Refresh => {
                    db.suppress_remote_notification(remote, &notification.admission_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Ok(());
                }
                SubscriptionAuthorization::Rejected { reason } => {
                    db.suppress_remote_notification(remote, &notification.admission_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    db.end_remote_subscription(&remote.reference, remote.subscription_id, reason)
                        .await
                        .map_err(|error| error.to_string())?;
                    continue;
                }
            }
        }
        let literal = SubscriptionNotificationLiteral {
            reference: &notification.reference,
            subscription_id: notification.subscription_id,
            delivery: &notification.content,
        };
        let text = format!("Federated conversation subscription notification (terminal execution facts or an explicit retention gap; not task completion). {}", serde_json::to_string(&literal).map_err(|error| error.to_string())?);
        let outcome = service
            .send(crate::send_chat_service::SendChatRequest {
                conversation_id: target.clone(),
                origin: phoenix_core::domain::db_schema::InputOrigin::SubscriptionEvent {
                    event_id: notification.admission_id.clone(),
                },
                text,
                message_id: notification.admission_id,
                images: vec![],
                files: vec![],
                user_agent: None,
                expansion_policy: crate::send_chat_service::MessageExpansionPolicy::LiteralText,
            })
            .await
            .map_err(|error| error.to_string())?;
        match outcome {
            crate::send_chat_service::SendChatOutcome::Delivered
            | crate::send_chat_service::SendChatOutcome::AlreadyPersisted
            | crate::send_chat_service::SendChatOutcome::QueuedAsSteering => {}
            crate::send_chat_service::SendChatOutcome::Rejected { message, .. } => {
                return Err(message)
            }
        }
        admitted_cursor = through;
    }
    Ok(())
}

pub(crate) const SYNC_PASS_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct SubscriptionSync {
    pending: Option<tokio::task::JoinHandle<()>>,
}

impl SubscriptionSync {
    pub(crate) async fn sync_pass(&mut self, runtime: &Arc<RuntimeManager>) {
        let Ok(owner) = runtime.acquire_local_authority_pass() else {
            if let Some(pending) = self.pending.take() {
                pending.abort();
            }
            return;
        };
        let pending = self.pending.get_or_insert_with(|| {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                let _owner = owner;
                sync_pass_inner(&runtime).await;
            })
        });
        match tokio::time::timeout(SYNC_PASS_BUDGET, pending).await {
            Ok(result) => {
                self.pending = None;
                if let Err(error) = result {
                    tracing::warn!(%error, "federated subscription sync task failed");
                }
            }
            Err(_) => {
                tracing::debug!(
                    "federated subscription sync budget exhausted; in-flight work retained"
                );
            }
        }
    }
}

impl Drop for SubscriptionSync {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.abort();
        }
    }
}

async fn sync_pass_inner(runtime: &Arc<RuntimeManager>) {
    let db = runtime.db();
    match db.next_remote_subscription_sync_batch(4).await {
        Ok(rows) => {
            for remote in rows {
                if let Err(error) = sync_once(db, runtime, &remote).await {
                    tracing::debug!(%error, "bounded federated subscription sync deferred");
                }
            }
        }
        Err(error) => tracing::warn!(%error, "remote subscription discovery failed"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct OwnerSubscriptionRequest {
    pub instance: String,
    pub product_conversation_id: ProductConversationId,
    pub subscription_id: Uuid,
}
pub async fn owner_create(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<OwnerSubscriptionRequest>,
) -> Response {
    match create_saved_peer_subscription(
        &state.db,
        request.instance,
        request.product_conversation_id,
        Some(request.subscription_id),
    )
    .await
    {
        Ok(remote) => Json(remote).into_response(),
        Err(error) => {
            tracing::debug!(%error, "owner subscription create failed");
            rejected(StatusCode::CONFLICT)
        }
    }
}
pub async fn owner_list(_: OwnerAuthenticated, State(state): State<AppState>) -> Response {
    match state.db.remote_subscriptions(None, 1024).await {
        Ok(rows) => Json(rows).into_response(),
        Err(_) => rejected(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
pub async fn owner_sync(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<OwnerSubscriptionRequest>,
) -> Response {
    let peer = match InstanceSelector::parse(request.instance) {
        Ok(selector) => match state.db.resolve_federation_peer(&selector).await {
            Ok(peer) => peer,
            Err(_) => return rejected(StatusCode::NOT_FOUND),
        },
        Err(_) => return rejected(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let reference = SubscriptionReference {
        instance_id: peer.peer_instance_id,
        product_conversation_id: request.product_conversation_id,
    };
    match state
        .db
        .remote_subscription_by_id(peer.peer_instance_id, request.subscription_id)
        .await
    {
        Ok(Some(remote)) if remote.reference == reference => {
            match sync_once(&state.db, &state.runtime, &remote).await {
                Ok(()) => Json(super::openapi::SubscriptionSyncReceipt { synced_once: true })
                    .into_response(),
                Err(error) => {
                    tracing::debug!(%error, "owner subscription sync failed");
                    rejected(StatusCode::BAD_GATEWAY)
                }
            }
        }
        _ => rejected(StatusCode::NOT_FOUND),
    }
}
pub async fn owner_unsubscribe(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    Json(request): Json<OwnerSubscriptionRequest>,
) -> Response {
    let Ok(selector) = InstanceSelector::parse(request.instance.clone()) else {
        return rejected(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let Ok(peer) = state.db.resolve_federation_peer(&selector).await else {
        return rejected(StatusCode::NOT_FOUND);
    };
    let reference = SubscriptionReference {
        instance_id: peer.peer_instance_id,
        product_conversation_id: request.product_conversation_id.clone(),
    };
    let Ok(Some(remote)) = state
        .db
        .remote_subscription_by_id(peer.peer_instance_id, request.subscription_id)
        .await
    else {
        return rejected(StatusCode::NOT_FOUND);
    };
    if remote.reference != reference {
        return rejected(StatusCode::NOT_FOUND);
    }
    match unsubscribe_remote(&state.db, &peer, &remote).await {
        Ok(receipt) => Json(receipt).into_response(),
        Err(error) => {
            tracing::debug!(%error, "owner unsubscribe failed before receipt");
            rejected(StatusCode::BAD_GATEWAY)
        }
    }
}

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct NotificationQuery {
    pub destination_instance_id: InstanceId,
    pub subscription_id: Uuid,
    pub after: Option<SubscriptionCursor>,
}
pub async fn owner_notifications(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<NotificationQuery>,
) -> Response {
    let remote = match state
        .db
        .remote_subscription_by_id(query.destination_instance_id, query.subscription_id)
        .await
    {
        Ok(Some(remote)) => remote,
        Ok(None) => return rejected(StatusCode::NOT_FOUND),
        Err(_) => return rejected(StatusCode::INTERNAL_SERVER_ERROR),
    };
    match state
        .db
        .remote_notification_history(&remote, query.after.unwrap_or(SubscriptionCursor::START))
        .await
    {
        Ok(records) => Json(records).into_response(),
        Err(_) => rejected(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
