use super::TestInstance;
use crate::api::federation::{
    send_remote_authored_conversation_message, send_remote_conversation_message,
    RemoteAuthoredSendMessageRequest, RemoteMessageAcceptance, RemoteMessageText,
    RemoteSendDisposition, RemoteSendMessageRequest, RemoteSendMessageResponse, RemoteSendReceipt,
    RemoteSendRejectionCode, RemoteSendTarget, RemoteSenderAttestation,
};
use phoenix_core::domain::{
    close::TranscriptConversationId,
    db_schema::{InputOrigin, MessageContent, SourceToolCall},
    instance_identity::InstanceId,
    sm_event::PreparedDirectTurnPayload,
};
use std::sync::Arc;

async fn ordinary(instance: &TestInstance, id: &str) -> crate::db::Conversation {
    instance
        .state
        .db
        .create_conversation(id, id, "/tmp", true, None, None)
        .await
        .unwrap()
}

fn request(destination: InstanceId, target: RemoteSendTarget) -> RemoteSendMessageRequest {
    RemoteSendMessageRequest {
        destination_instance_id: destination,
        target,
        message_id: uuid::Uuid::new_v4(),
        message: RemoteMessageText::parse("literal @conv:do-not-expand".into()).unwrap(),
    }
}

fn acceptance(response: &RemoteSendMessageResponse) -> &RemoteMessageAcceptance {
    match &response.receipt {
        RemoteSendReceipt::Accepted { acceptance }
        | RemoteSendReceipt::Duplicate { acceptance } => acceptance,
        receipt @ RemoteSendReceipt::Rejected { .. } => {
            panic!("expected acceptance-only receipt, got {receipt:?}")
        }
    }
}

async fn coordinator_id(instance: &TestInstance) -> String {
    instance
        .state
        .db
        .coordinator_conversation_id()
        .await
        .unwrap()
        .unwrap()
}

fn send_tool(caller: &TestInstance) -> Arc<dyn crate::tools::Tool> {
    crate::coordinator_tools::writing_tools(
        crate::api::global_read::GlobalReadService::new(
            caller.state.db.clone(),
            caller.state.message_retriever.clone(),
        ),
        Arc::new(crate::send_chat_service::SendChatApplicationService::new(
            caller.state.db.clone(),
            caller.state.runtime.clone(),
        )),
    )
    .into_tools()
    .find(|tool| tool.name() == "send_conversation_message")
    .unwrap()
}

fn context(caller: &TestInstance, source: &str) -> crate::tools::ToolContext {
    crate::tools::ToolContext::new_without_filesystem(
        tokio_util::sync::CancellationToken::new(),
        source.to_string(),
        Arc::new(crate::tools::BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        caller.state.llm_registry.clone(),
        phoenix_terminal::ActiveTerminals::new(),
        Arc::new(crate::tools::TmuxRegistry::new()),
    )
    .with_svg_assistant_message_id("source-assistant")
    .with_tool_use_id("source-send-call")
}

async fn stored_payload(receiver: &TestInstance, id: &str) -> PreparedDirectTurnPayload {
    let turn = receiver
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(id.to_string()))
        .await
        .unwrap()
        .unwrap();
    PreparedDirectTurnPayload::from_exact_bytes(turn.prepared.payload()).unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn https_send_requires_peer_auth_and_checks_destination_and_closed_body() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let target = ordinary(&receiver, "send-auth-target").await;
    let destination = receiver.state.db.instance_id().await.unwrap();
    let mut request = request(
        destination,
        RemoteSendTarget::Transcript {
            transcript_id: TranscriptConversationId::parse(target.id.clone()).unwrap(),
        },
    );
    let endpoint = receiver.endpoint("api/federation/peer/send-conversation-message");
    let response = receiver
        .client
        .post(&endpoint)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    let peer = caller.connect_to(&receiver).await;
    request.destination_instance_id = InstanceId::new();
    let response = receiver
        .client
        .post(&endpoint)
        .bearer_auth(peer.bearer_credential.expose())
        .json(&request)
        .send()
        .await
        .unwrap();
    let receipt: RemoteSendMessageResponse = response.json().await.unwrap();
    assert!(matches!(
        receipt.receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::DestinationMismatch,
            ..
        }
    ));
    assert!(receiver
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(target.id.clone()))
        .await
        .unwrap()
        .is_none());
    request.destination_instance_id = destination;
    let valid = serde_json::to_value(&request).unwrap();
    for (field, value) in [
        ("source_instance_id", serde_json::json!(InstanceId::new())),
        ("caller_instance_id", serde_json::json!(InstanceId::new())),
        ("instance_id", serde_json::json!(InstanceId::new())),
        (
            "sender",
            serde_json::json!({"kind":"coordinator", "product_conversation_id":"forged-coordinator", "transcript_id":"forged-source", "source_call":{"message_id":"forged-message", "tool_use_id":"forged-call"}}),
        ),
        (
            "sender",
            serde_json::json!({"kind":"conversation", "product_conversation_id":"forged-conversation", "transcript_id":"forged-source", "source_call":{"message_id":"forged-message", "tool_use_id":"forged-call"}}),
        ),
        ("sender", serde_json::json!({"kind":"remote_api"})),
        ("message", serde_json::json!("x".repeat(32 * 1024 + 1))),
        ("message_id", serde_json::json!("invalid")),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        if field == "sender" {
            invalid["target"] = serde_json::json!({"kind":"coordinator"});
        }
        let response = receiver
            .client
            .post(&endpoint)
            .bearer_auth(peer.bearer_credential.expose())
            .json(&invalid)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        if field == "sender" {
            assert!(response
                .text()
                .await
                .unwrap()
                .contains("unknown field `sender`"));
        }
    }
    let mut oversized = valid;
    oversized["message"] = serde_json::json!("x".repeat(257 * 1024));
    let response = receiver
        .client
        .post(&endpoint)
        .bearer_auth(peer.bearer_credential.expose())
        .json(&oversized)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    let response = receiver
        .client
        .post(&endpoint)
        .bearer_auth(peer.bearer_credential.expose())
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let response: RemoteSendMessageResponse = response.json().await.unwrap();
    assert!(matches!(
        response.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    assert_eq!(
        stored_payload(&receiver, &target.id).await.submitted.origin,
        InputOrigin::RemoteApi {
            instance_id: caller.state.db.instance_id().await.unwrap()
        }
    );
    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
async fn https_authored_send_rejects_api_senders_and_instance_claims() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let source = ordinary(&caller, "authored-source").await;
    let target = ordinary(&receiver, "authored-target").await;
    let peer = caller.connect_to(&receiver).await;
    let api_request = request(
        peer.peer_instance_id,
        RemoteSendTarget::Transcript {
            transcript_id: TranscriptConversationId::parse(target.id.clone()).unwrap(),
        },
    );
    let sender = RemoteSenderAttestation::from_conversation(
        source,
        SourceToolCall {
            message_id: "source-assistant".into(),
            tool_use_id: "source-send-call".into(),
        },
    )
    .unwrap();
    let authored = RemoteAuthoredSendMessageRequest::from_message(api_request.clone(), sender);
    let endpoint = receiver.endpoint("api/federation/peer/send-authored-conversation-message");
    assert_eq!(
        receiver
            .client
            .post(&endpoint)
            .json(&authored)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let valid = serde_json::to_value(authored).unwrap();
    let mut malformed = vec![serde_json::to_value(api_request).unwrap()];
    for sender in [
        serde_json::json!({"kind":"remote_api"}),
        serde_json::json!({"kind":"conversation", "product_conversation_id":"source", "transcript_id":"source", "source_call":null}),
        serde_json::json!({"kind":"coordinator", "product_conversation_id":"source", "transcript_id":"source"}),
        serde_json::json!({"kind":"conversation", "product_conversation_id":"source", "transcript_id":"source", "source_call":{"message_id":"half-call"}}),
        serde_json::json!({"kind":"conversation", "product_conversation_id":"source", "transcript_id":"source", "source_call":{"message_id":"m", "tool_use_id":"t", "instance_id":InstanceId::new()}}),
    ] {
        let mut invalid = valid.clone();
        invalid["sender"] = sender;
        malformed.push(invalid);
    }
    for field in [
        "source_instance_id",
        "caller_instance_id",
        "instance_id",
        "origin",
        "extra",
    ] {
        let mut invalid = valid.clone();
        invalid[field] = serde_json::json!(InstanceId::new());
        malformed.push(invalid);
        let mut invalid = valid.clone();
        invalid["sender"][field] = serde_json::json!(InstanceId::new());
        malformed.push(invalid);
    }
    for invalid in malformed {
        let response = receiver
            .client
            .post(&endpoint)
            .bearer_auth(peer.bearer_credential.expose())
            .json(&invalid)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert!(receiver
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(target.id))
        .await
        .unwrap()
        .is_none());
    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn https_tool_same_local_and_remote_ids_are_not_self_and_replays_bind_instance() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let source = ordinary(&caller, "shared-transcript").await;
    let target = ordinary(&receiver, "shared-transcript").await;
    let peer = caller.connect_to(&receiver).await;
    let caller_id = caller.state.db.instance_id().await.unwrap();
    let id = uuid::Uuid::new_v4();
    let input = serde_json::json!({
        "instance":peer.peer_instance_id.to_string(),
        "target":format!("@conv:{}", target.product_conversation_id),
        "message":"literal @conv:do-not-expand",
        "message_id":id,
    });
    let tool = send_tool(&caller);
    let output = tool.run(input.clone(), context(&caller, &source.id)).await;
    assert!(output.is_success(), "{}", output.output());
    let response: RemoteSendMessageResponse = serde_json::from_str(output.output()).unwrap();
    assert!(matches!(
        response.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    assert_eq!(
        acceptance(&response).accepting_transcript.as_str(),
        target.id
    );
    assert_eq!(
        acceptance(&response).stable_destination.instance_id,
        peer.peer_instance_id
    );
    assert_eq!(
        acceptance(&response)
            .stable_destination
            .product_conversation_id,
        target.product_conversation_id
    );
    assert_eq!(
        acceptance(&response).disposition,
        RemoteSendDisposition::Delivered
    );
    let accepted = stored_payload(&receiver, &target.id).await;
    assert_eq!(accepted.submitted.text, "literal @conv:do-not-expand");
    assert_eq!(accepted.delivery.text, accepted.submitted.text);
    assert!(
        matches!(&accepted.submitted.origin, InputOrigin::RemoteConversation { instance_id, product_conversation_id, transcript_id, source_call:Some(call) } if *instance_id == caller_id && product_conversation_id == &source.product_conversation_id && transcript_id == &source.id && call.message_id == "source-assistant" && call.tool_use_id == "source-send-call")
    );
    let output = tool
        .run(
            input,
            context(&caller, &source.id).with_tool_use_id("retry-call"),
        )
        .await;
    let replay: RemoteSendMessageResponse = serde_json::from_str(output.output()).unwrap();
    assert!(matches!(
        replay.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(acceptance(&response), acceptance(&replay));
    let mut api_replay = request(peer.peer_instance_id, response.target.clone());
    api_replay.message_id = id;
    let api_replay = send_remote_conversation_message(&caller.state.db, &peer, &api_replay)
        .await
        .unwrap();
    assert!(matches!(
        api_replay.receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::IdempotencyConflict,
            ..
        }
    ));
    assert_eq!(
        stored_payload(&receiver, &target.id).await.submitted.origin,
        accepted.submitted.origin
    );

    receiver
        .state
        .db
        .revoke_federation_enrollment(caller_id)
        .await
        .unwrap();
    let failed = tool
        .run(
            serde_json::json!({
                "instance":peer.peer_instance_id.to_string(),
                "target":"@transcript:shared-transcript",
                "message":"must not fall back to caller's identically named conversation",
                "message_id":uuid::Uuid::new_v4(),
            }),
            context(&caller, "shared-transcript"),
        )
        .await;
    assert!(!failed.is_success());
    assert!(failed.output().contains("no local fallback"));

    let other_caller = TestInstance::start().await;
    let other_peer = other_caller.connect_to(&receiver).await;
    let sender = RemoteSenderAttestation::from_conversation(
        source,
        SourceToolCall {
            message_id: "source-assistant".into(),
            tool_use_id: "source-send-call".into(),
        },
    )
    .unwrap();
    let mut conflict = request(peer.peer_instance_id, response.target);
    conflict.message_id = id;
    let conflict = RemoteAuthoredSendMessageRequest::from_message(conflict, sender);
    let changed_instance =
        send_remote_authored_conversation_message(&other_caller.state.db, &other_peer, &conflict)
            .await
            .unwrap();
    assert!(matches!(
        changed_instance.receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::IdempotencyConflict,
            ..
        }
    ));
    assert!(caller
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(target.id))
        .await
        .unwrap()
        .is_none());
    other_caller.stop().await;
    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn https_stable_follows_current_member_but_exact_does_not_retarget() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let predecessor = ordinary(&receiver, "send-predecessor").await;
    receiver
        .state
        .db
        .add_message(
            "send-summary",
            &predecessor.id,
            &MessageContent::continuation("handoff"),
            None,
            None,
        )
        .await
        .unwrap();
    receiver
        .state
        .db
        .update_conversation_state(
            &predecessor.id,
            &crate::db::ConvState::ContextExhausted {
                summary: "handoff".into(),
            },
        )
        .await
        .unwrap();
    let crate::db::ContinueOutcome::Created(successor) = receiver
        .state
        .db
        .continue_conversation(&predecessor.id)
        .await
        .unwrap()
    else {
        panic!("expected successor")
    };
    let peer = caller.connect_to(&receiver).await;
    let child = receiver
        .state
        .db
        .create_conversation(
            "remote-child",
            "child",
            "/tmp",
            true,
            Some(&successor.id),
            None,
        )
        .await
        .unwrap();
    let child_request = request(
        peer.peer_instance_id,
        RemoteSendTarget::Transcript {
            transcript_id: TranscriptConversationId::parse(child.id).unwrap(),
        },
    );
    assert!(matches!(
        send_remote_conversation_message(&caller.state.db, &peer, &child_request)
            .await
            .unwrap()
            .receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::InvalidTarget,
            ..
        }
    ));
    let stable = request(
        peer.peer_instance_id,
        RemoteSendTarget::ProductConversation {
            product_conversation_id: predecessor.product_conversation_id.clone(),
        },
    );
    let response = send_remote_conversation_message(&caller.state.db, &peer, &stable)
        .await
        .unwrap();
    assert_eq!(
        acceptance(&response).accepting_transcript.as_str(),
        successor.id
    );
    assert_eq!(
        acceptance(&response)
            .stable_destination
            .product_conversation_id,
        predecessor.product_conversation_id
    );
    let exact = request(
        peer.peer_instance_id,
        RemoteSendTarget::Transcript {
            transcript_id: TranscriptConversationId::parse(predecessor.id.clone()).unwrap(),
        },
    );
    let exact_response = send_remote_conversation_message(&caller.state.db, &peer, &exact)
        .await
        .unwrap();
    assert!(matches!(
        exact_response.receipt,
        RemoteSendReceipt::Rejected { .. }
    ));
    assert!(receiver
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(predecessor.id))
        .await
        .unwrap()
        .is_none());
    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn https_only_remote_coordinator_may_message_coordinator_and_archive_still_fences() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let source = ordinary(&caller, "ordinary-source").await;
    let peer = caller.connect_to(&receiver).await;
    let coordinator_id = receiver
        .state
        .db
        .coordinator_conversation_id()
        .await
        .unwrap()
        .unwrap();
    let caller_id = caller.state.db.instance_id().await.unwrap();
    for target in [
        RemoteSendTarget::Coordinator {},
        RemoteSendTarget::Transcript {
            transcript_id: TranscriptConversationId::parse(coordinator_id.clone()).unwrap(),
        },
    ] {
        let denied = request(peer.peer_instance_id, target);
        let api_response = send_remote_conversation_message(&caller.state.db, &peer, &denied)
            .await
            .unwrap();
        let sender = RemoteSenderAttestation::from_conversation(
            source.clone(),
            SourceToolCall {
                message_id: "source".into(),
                tool_use_id: "call".into(),
            },
        )
        .unwrap();
        let denied = RemoteAuthoredSendMessageRequest::from_message(denied, sender);
        let authored_response =
            send_remote_authored_conversation_message(&caller.state.db, &peer, &denied)
                .await
                .unwrap();
        for response in [api_response, authored_response] {
            assert!(matches!(
                response.receipt,
                RemoteSendReceipt::Rejected {
                    code: RemoteSendRejectionCode::SenderNotAllowed,
                    ..
                }
            ));
        }
    }
    let caller_coordinator_id = caller
        .state
        .db
        .coordinator_conversation_id()
        .await
        .unwrap()
        .unwrap();
    let output = send_tool(&caller).run(serde_json::json!({"instance":peer.peer_instance_id.to_string(), "target":"@coordinator", "message":"peer coordination, not completion", "message_id":uuid::Uuid::new_v4()}), context(&caller, &caller_coordinator_id)).await;
    assert!(output.is_success(), "{}", output.output());
    let response: RemoteSendMessageResponse = serde_json::from_str(output.output()).unwrap();
    assert!(matches!(
        response.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    assert_eq!(
        acceptance(&response).accepting_transcript.as_str(),
        coordinator_id
    );
    let stored = stored_payload(&receiver, &coordinator_id).await;
    let source_coordinator = caller
        .state
        .db
        .get_conversation(&caller_coordinator_id)
        .await
        .unwrap();
    assert!(
        matches!(&stored.submitted.origin, InputOrigin::RemoteCoordinator { instance_id, product_conversation_id, transcript_id, source_call:Some(call) } if *instance_id == caller_id && product_conversation_id == &source_coordinator.product_conversation_id && transcript_id == &caller_coordinator_id && call.message_id == "source-assistant" && call.tool_use_id == "source-send-call")
    );
    let mut exact_coordinator = request(
        peer.peer_instance_id,
        RemoteSendTarget::Transcript {
            transcript_id: TranscriptConversationId::parse(coordinator_id.clone()).unwrap(),
        },
    );
    exact_coordinator.message_id = response.message_id;
    exact_coordinator.message =
        RemoteMessageText::parse("peer coordination, not completion".into()).unwrap();
    let sender = RemoteSenderAttestation::from_conversation(
        source_coordinator.clone(),
        SourceToolCall {
            message_id: "retry-assistant".into(),
            tool_use_id: "retry-call".into(),
        },
    )
    .unwrap();
    let exact_coordinator =
        RemoteAuthoredSendMessageRequest::from_message(exact_coordinator, sender);
    let exact_response =
        send_remote_authored_conversation_message(&caller.state.db, &peer, &exact_coordinator)
            .await
            .unwrap();
    assert!(matches!(
        exact_response.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(
        acceptance(&exact_response).accepting_transcript.as_str(),
        coordinator_id
    );
    assert_eq!(
        stored_payload(&receiver, &coordinator_id)
            .await
            .submitted
            .origin,
        stored.submitted.origin
    );
    receiver
        .state
        .db
        .set_legacy_conversation_archived(&coordinator_id)
        .await
        .unwrap();
    let archived = request(
        peer.peer_instance_id,
        RemoteSendTarget::Transcript {
            transcript_id: TranscriptConversationId::parse(coordinator_id).unwrap(),
        },
    );
    let sender = RemoteSenderAttestation::from_conversation(
        source_coordinator,
        SourceToolCall {
            message_id: "source".into(),
            tool_use_id: "call".into(),
        },
    )
    .unwrap();
    let archived = RemoteAuthoredSendMessageRequest::from_message(archived, sender);
    let response = send_remote_authored_conversation_message(&caller.state.db, &peer, &archived)
        .await
        .unwrap();
    assert!(matches!(
        response.receipt,
        RemoteSendReceipt::Rejected { .. }
    ));
    caller.stop().await;
    receiver.stop().await;
}

pub(super) async fn materialize_received(
    instance: &TestInstance,
    transcript: &str,
) -> crate::db::Message {
    use phoenix_db::workflow::{
        ClaimAuthoritativeTurnInput, MaterializeAuthoritativeTurnInput,
        MaterializeAuthoritativeTurnOutcome,
    };
    use phoenix_workflow::{ConversationAuthority, LeaseExpiry, ProcessIncarnation, Timestamp};
    let repo = instance.state.db.workflow_repository();
    let turn = repo
        .load_active_runtime_turn(&ConversationAuthority(transcript.into()))
        .await
        .unwrap()
        .unwrap();
    let now = chrono::Utc::now();
    let timestamp = Timestamp(now.timestamp_micros().try_into().unwrap());
    let workflow_id = repo.workflow_id_for_turn(turn.id).await.unwrap().unwrap();
    let claim = repo
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id: turn.id,
            workflow_id,
            process_incarnation: ProcessIncarnation(1),
            now: timestamp,
            lease_until: LeaseExpiry(timestamp.0 + 60_000_000),
        })
        .await
        .unwrap();
    let outcome = repo
        .materialize_authoritative_turn(&MaterializeAuthoritativeTurnInput {
            turn_id: turn.id,
            authority: claim.authority.unwrap(),
            prepared: PreparedDirectTurnPayload::from_exact_bytes(turn.prepared.payload()).unwrap(),
            sequence_id: instance
                .state
                .db
                .get_last_sequence_id(transcript)
                .await
                .unwrap()
                + 1,
            created_at: timestamp,
            accepted_state: crate::db::ConvState::LlmRequesting { attempt: 1 },
            state_updated_at: now,
            now: timestamp,
        })
        .await
        .established()
        .unwrap();
    match outcome {
        MaterializeAuthoritativeTurnOutcome::Materialized(row)
        | MaterializeAuthoritativeTurnOutcome::ExactReplay(row)
        | MaterializeAuthoritativeTurnOutcome::ClassifiedCommitted(row) => *row.message,
        other @ (MaterializeAuthoritativeTurnOutcome::NotCommitted
        | MaterializeAuthoritativeTurnOutcome::StaleAuthority
        | MaterializeAuthoritativeTurnOutcome::CommandRejected(_)) => {
            panic!("expected authoritative message projection: {other:?}")
        }
    }
}

#[tokio::test]
async fn https_coordinator_retry_after_continuation_returns_original_acceptance() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let peer = caller.connect_to(&receiver).await;
    let source_id = coordinator_id(&caller).await;
    let source = caller.state.db.get_conversation(&source_id).await.unwrap();
    let original = coordinator_id(&receiver).await;
    let send = request(peer.peer_instance_id, RemoteSendTarget::Coordinator {});
    let sender = RemoteSenderAttestation::from_conversation(
        source,
        SourceToolCall {
            message_id: "source-assistant".into(),
            tool_use_id: "source-send".into(),
        },
    )
    .unwrap();
    let send = RemoteAuthoredSendMessageRequest::from_message(send, sender);
    let accepted = send_remote_authored_conversation_message(&caller.state.db, &peer, &send)
        .await
        .unwrap();
    assert!(matches!(
        accepted.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    let message = materialize_received(&receiver, &original).await;
    assert_eq!(
        message.message_id,
        format!("{original}:{}", send.message_id)
    );
    receiver
        .state
        .db
        .add_message(
            "coordinator-handoff",
            &original,
            &MessageContent::continuation("handoff"),
            None,
            None,
        )
        .await
        .unwrap();
    receiver
        .state
        .db
        .update_conversation_state(
            &original,
            &crate::db::ConvState::ContextExhausted {
                summary: "handoff".into(),
            },
        )
        .await
        .unwrap();
    let crate::db::ContinueOutcome::Created(successor) = receiver
        .state
        .db
        .continue_conversation(&original)
        .await
        .unwrap()
    else {
        panic!("expected Coordinator continuation")
    };
    assert_eq!(
        receiver
            .state
            .db
            .coordinator_conversation_id()
            .await
            .unwrap()
            .as_deref(),
        Some(successor.id.as_str())
    );
    let replay = send_remote_authored_conversation_message(&caller.state.db, &peer, &send)
        .await
        .unwrap();
    assert!(matches!(
        replay.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(acceptance(&replay), acceptance(&accepted));
    assert_eq!(acceptance(&replay).accepting_transcript.as_str(), original);
    assert!(receiver
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(successor.id))
        .await
        .unwrap()
        .is_none());
    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn https_send_then_search_resolve_and_rename_preserve_foreign_sender_and_exact_evidence() {
    use crate::api::{
        federation::{resolve_remote_reference, search_remote_conversations, RemoteSendNavigation},
        global_read::QualifiedSearchNavigation,
        product_conversations::{exact_transcript_route, message_fragment, product_route},
    };
    use phoenix_core::domain::conversation_reference::{
        ConversationReferenceTarget, MessageId, QualifiedConversationReference,
    };
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let source = ordinary(&caller, "colliding-transcript").await;
    let target = ordinary(&receiver, "colliding-transcript").await;
    assert!(uuid::Uuid::parse_str(target.product_conversation_id.as_str()).is_ok());
    let caller_id = caller.state.db.instance_id().await.unwrap();
    let peer = caller.connect_to(&receiver).await;
    let stable_ref = format!("@conv:{}", target.product_conversation_id);
    let mut send = request(
        peer.peer_instance_id,
        RemoteSendTarget::ProductConversation {
            product_conversation_id: target.product_conversation_id.clone(),
        },
    );
    send.message = RemoteMessageText::parse("federationreceivedmarker".into()).unwrap();
    let sender = RemoteSenderAttestation::from_conversation(
        source.clone(),
        SourceToolCall {
            message_id: "source-assistant".into(),
            tool_use_id: "source-send".into(),
        },
    )
    .unwrap();
    let send = RemoteAuthoredSendMessageRequest::from_message(send, sender);
    let accepted = send_remote_authored_conversation_message(&caller.state.db, &peer, &send)
        .await
        .unwrap();
    assert!(matches!(
        accepted.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    let message = materialize_received(&receiver, &target.id).await;
    let target_transcript = TranscriptConversationId::parse(&target.id).unwrap();
    let evidence = QualifiedConversationReference::new(
        peer.peer_instance_id,
        ConversationReferenceTarget::Message {
            transcript_id: target_transcript.clone(),
            message_id: MessageId::parse(&message.message_id).unwrap(),
        },
    );
    let expected_source = format!(
        "{}{}",
        exact_transcript_route(&target_transcript),
        message_fragment(&message.message_id)
    );
    let expected_source_url = peer
        .base_url
        .navigation_url(&expected_source)
        .unwrap()
        .as_url()
        .to_string();
    assert_eq!(
        acceptance(&accepted).accepting_message_navigation,
        RemoteSendNavigation::Peer {
            url: expected_source_url.clone()
        }
    );
    let old_resolution = resolve_remote_reference(&caller.state.db, &peer, stable_ref.clone())
        .await
        .unwrap();
    assert_eq!(
        old_resolution.reference.target,
        ConversationReferenceTarget::ProductConversation {
            product_conversation_id: target.product_conversation_id.clone()
        }
    );
    receiver
        .state
        .db
        .set_ordinary_product_conversation_title(&target.id, "Renamed receiver conversation")
        .await
        .unwrap();
    let renamed = receiver
        .state
        .db
        .get_conversation(&target.id)
        .await
        .unwrap();
    assert_eq!(
        renamed.product_conversation_id,
        target.product_conversation_id
    );
    let current_path = product_route(
        renamed.slug.as_deref(),
        target.product_conversation_id.as_str(),
    );
    let current_url = peer
        .base_url
        .navigation_url(&current_path)
        .unwrap()
        .as_url()
        .to_string();
    let current_resolution = resolve_remote_reference(&caller.state.db, &peer, stable_ref)
        .await
        .unwrap();
    assert_eq!(current_resolution.reference, old_resolution.reference);
    assert_ne!(current_resolution.href, old_resolution.href);
    assert_eq!(
        current_resolution.href.as_deref(),
        Some(current_url.as_str())
    );
    let replay = send_remote_authored_conversation_message(&caller.state.db, &peer, &send)
        .await
        .unwrap();
    assert!(matches!(
        replay.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(
        acceptance(&replay).stable_destination,
        acceptance(&accepted).stable_destination
    );
    assert_eq!(
        acceptance(&replay).stable_navigation,
        RemoteSendNavigation::Peer {
            url: current_url.clone()
        }
    );
    assert_eq!(
        acceptance(&replay).accepting_message_navigation,
        acceptance(&accepted).accepting_message_navigation
    );
    let hits = search_remote_conversations(&caller.state.db, &peer, "federationreceivedmarker")
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source, evidence);
    assert!(
        matches!(&hits[0].source_navigation, Some(QualifiedSearchNavigation::Peer { url }) if url == &expected_source_url)
    );
    assert!(
        matches!(&hits[0].stable_navigation, Some(QualifiedSearchNavigation::Peer { url }) if url == &current_url)
    );
    let sender = hits[0].sender.as_ref().unwrap();
    assert_eq!(sender.stable.instance_id, caller_id);
    assert_eq!(sender.exact.instance_id, caller_id);
    assert_eq!(
        sender.stable.target,
        ConversationReferenceTarget::ProductConversation {
            product_conversation_id: source.product_conversation_id
        }
    );
    assert_eq!(
        sender.exact.target,
        ConversationReferenceTarget::Transcript {
            transcript_id: TranscriptConversationId::parse(&source.id).unwrap()
        }
    );
    let resolved = resolve_remote_reference(
        &caller.state.db,
        &peer,
        format!("@transcript:{}#message-{}", target.id, message.message_id),
    )
    .await
    .unwrap();
    assert_eq!(resolved.reference, evidence);
    assert_eq!(resolved.href.as_deref(), Some(expected_source_url.as_str()));
    assert_eq!(
        resolved.related_references,
        vec![sender.exact.clone(), sender.stable.clone()]
    );
    assert!(resolved
        .related_references
        .iter()
        .all(|reference| reference.instance_id != peer.peer_instance_id));
    let serialized = serde_json::to_string(&replay).unwrap();
    assert!(!serialized.contains(peer.bearer_credential.expose()));
    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn shared_selectors_never_fall_back_on_unknown_tls_auth_or_unavailable_peers() {
    use phoenix_core::domain::instance_identity::PeerTlsTrust;
    use serde_json::{json, Value};
    let caller = TestInstance::start().await;
    let mut receiver = TestInstance::start().await;
    let source = ordinary(&caller, "selector-source").await;
    let local_target = caller
        .state
        .db
        .create_conversation(
            "selector-target",
            "localsentinelmarker",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    caller
        .state
        .db
        .add_message(
            "local-sentinel-message",
            &local_target.id,
            &MessageContent::user("localsentinelmarker"),
            None,
            None,
        )
        .await
        .unwrap();
    ordinary(&receiver, "selector-target").await;
    let peer = caller.connect_to(&receiver).await;
    let tools = crate::coordinator_tools::tools(
        crate::api::global_read::GlobalReadService::new(
            caller.state.db.clone(),
            caller.state.message_retriever.clone(),
        ),
        Arc::new(crate::send_chat_service::SendChatApplicationService::new(
            caller.state.db.clone(),
            caller.state.runtime.clone(),
        )),
        caller.state.runtime.work_scope_load().clone(),
        caller.state.db.clone(),
    );
    let cases = [
        (
            "query_database",
            json!({"sql":"SELECT title FROM conversations WHERE id = 'selector-target'"}),
        ),
        (
            "read_conversation",
            json!({"conversation_id":"@transcript:selector-target"}),
        ),
        (
            "resolve_reference",
            json!({"reference":"@transcript:selector-target"}),
        ),
        (
            "send_conversation_message",
            json!({"target":"@transcript:selector-target", "message":"must not reach local admission", "message_id":uuid::Uuid::new_v4()}),
        ),
    ];
    for (name, input) in &cases[..3] {
        let tool = tools.iter().find(|tool| tool.name() == *name).unwrap();
        let local = tool.run(input.clone(), context(&caller, &source.id)).await;
        assert!(local.is_success(), "local {name}: {}", local.output());
        assert!(
            local.output().contains("localsentinelmarker"),
            "local {name}: {}",
            local.output()
        );
    }
    let query = tools
        .iter()
        .find(|tool| tool.name() == "query_database")
        .unwrap();
    let mut remote_query = cases[0].1.clone();
    remote_query["instance"] = json!("isolated receiver");
    let output = query.run(remote_query, context(&caller, &source.id)).await;
    assert!(output.is_success(), "remote query: {}", output.output());
    let response: crate::api::federation::RemoteQueryDatabaseResponse =
        serde_json::from_str(output.output()).unwrap();
    assert_eq!(response.destination_instance_id, peer.peer_instance_id);
    assert!(!output.output().contains("localsentinelmarker"));
    for selector in [
        json!("missing-peer"),
        json!(InstanceId::new()),
        Value::Null,
        json!(false),
        json!(""),
    ] {
        for (name, base) in &cases {
            let tool = tools.iter().find(|tool| tool.name() == *name).unwrap();
            let mut input = base.clone();
            input["instance"] = selector.clone();
            let output = tool.run(input, context(&caller, &source.id)).await;
            assert!(
                !output.is_success(),
                "{name} fell back with {selector}: {}",
                output.output()
            );
            assert!(!output.output().contains("localsentinelmarker"));
        }
    }
    let invalid = query
        .run(
            json!({"sql":"SELECT 1", "instance_id":peer.peer_instance_id}),
            context(&caller, &source.id),
        )
        .await;
    assert!(!invalid.is_success());
    assert!(invalid.output().contains("unknown field"));
    for failure in ["tls", "auth", "unavailable"] {
        let trust = if failure == "tls" {
            PeerTlsTrust::PlatformRoots
        } else {
            peer.tls_trust.clone()
        };
        caller
            .state
            .db
            .save_federation_peer_connection(
                peer.peer_instance_id,
                "isolated receiver",
                &peer.base_url,
                &peer.bearer_credential,
                &trust,
            )
            .await
            .unwrap();
        if failure == "auth" {
            receiver
                .state
                .db
                .revoke_federation_enrollment(caller.state.db.instance_id().await.unwrap())
                .await
                .unwrap();
        }
        if failure == "unavailable" {
            receiver.shutdown.cancel();
            (&mut receiver.server).await.unwrap();
        }
        for (name, base) in &cases {
            let tool = tools.iter().find(|tool| tool.name() == *name).unwrap();
            let mut input = base.clone();
            input["instance"] = json!(peer.peer_instance_id);
            let output = tool.run(input, context(&caller, &source.id)).await;
            assert!(
                !output.is_success(),
                "{name} fell back on {failure}: {}",
                output.output()
            );
            assert!(!output.output().contains("localsentinelmarker"));
        }
    }
    assert!(caller
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(
            local_target.id.clone()
        ))
        .await
        .unwrap()
        .is_none());
    let send = tools
        .iter()
        .find(|tool| tool.name() == "send_conversation_message")
        .unwrap();
    let local_send = send
        .run(cases[3].1.clone(), context(&caller, &source.id))
        .await;
    assert!(
        local_send.is_success(),
        "omitted instance local send: {}",
        local_send.output()
    );
    assert!(caller
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(local_target.id))
        .await
        .unwrap()
        .is_some());
    caller.stop().await;
}
