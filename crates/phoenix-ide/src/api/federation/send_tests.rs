use super::*;
use phoenix_core::domain::{
    close::TranscriptConversationId,
    db_schema::{InputOrigin, SourceToolCall},
    product_conversation::ProductConversationId,
};
use serde_json::{json, Value};

fn request() -> RemoteSendMessageRequest {
    RemoteSendMessageRequest {
        destination_instance_id: InstanceId::new(),
        target: RemoteSendTarget::Transcript {
            transcript_id: TranscriptConversationId::parse("recipient").unwrap(),
        },
        message_id: uuid::Uuid::new_v4(),
        message: RemoteMessageText::parse("literal @conv:not-an-expansion".into()).unwrap(),
        sender: RemoteSenderAttestation::RemoteApi {},
    }
}

#[test]
fn remote_rejected_outcome_preserves_only_known_target_unavailable_code() {
    for (code, message, expected) in [
        (
            "target_unavailable",
            "unrelated diagnostic",
            RemoteSendRejectionCode::TargetUnavailable,
        ),
        (
            "unknown",
            "Conversation is archived and unavailable for messaging.",
            RemoteSendRejectionCode::AdmissionRejected,
        ),
        ("", "empty code", RemoteSendRejectionCode::AdmissionRejected),
        (
            "invalid_state",
            "ordinary rejection",
            RemoteSendRejectionCode::AdmissionRejected,
        ),
        (
            "steering_queue_full",
            "full queue",
            RemoteSendRejectionCode::AdmissionRejected,
        ),
        (
            "close_admission_fenced",
            "closing",
            RemoteSendRejectionCode::AdmissionRejected,
        ),
        (
            "target_unavailable ",
            "noncanonical code",
            RemoteSendRejectionCode::AdmissionRejected,
        ),
    ] {
        match remote_send_outcome_rejection(message, code) {
            RemoteSendReceipt::Rejected {
                code: actual,
                message: diagnostic,
            } => {
                assert_eq!(actual, expected);
                assert_eq!(diagnostic, message);
            }
            receipt
            @ (RemoteSendReceipt::Accepted { .. } | RemoteSendReceipt::Duplicate { .. }) => {
                panic!("unexpected non-rejection receipt: {receipt:?}")
            }
        }
    }
}

#[test]
fn remote_rejected_outcome_keeps_the_existing_512_unicode_scalar_diagnostic_bound() {
    let message = "\u{1f980}".repeat(513);
    for (code, expected) in [
        (
            "target_unavailable",
            RemoteSendRejectionCode::TargetUnavailable,
        ),
        ("unknown", RemoteSendRejectionCode::AdmissionRejected),
    ] {
        match remote_send_outcome_rejection(&message, code) {
            RemoteSendReceipt::Rejected {
                code: actual,
                message: diagnostic,
            } => {
                assert_eq!(actual, expected);
                assert_eq!(diagnostic, "\u{1f980}".repeat(512));
                assert_eq!(diagnostic.chars().count(), 512);
                assert_eq!(diagnostic.len(), 2048);
            }
            receipt
            @ (RemoteSendReceipt::Accepted { .. } | RemoteSendReceipt::Duplicate { .. }) => {
                panic!("unexpected non-rejection receipt: {receipt:?}")
            }
        }
    }
}

#[test]
fn send_request_is_closed_and_does_not_accept_a_source_instance() {
    let wire = serde_json::to_value(request()).unwrap();
    for field in [
        "source_instance_id",
        "caller_instance_id",
        "instance_id",
        "origin",
    ] {
        let mut invalid = wire.clone();
        invalid[field] = json!(InstanceId::new());
        assert!(serde_json::from_value::<RemoteSendMessageRequest>(invalid).is_err());
    }
    for target in [
        json!({"kind":"message", "transcript_id":"recipient", "message_id":"citation"}),
        json!({"kind":"coordinator", "instance_id":InstanceId::new()}),
    ] {
        let mut invalid = wire.clone();
        invalid["target"] = target;
        assert!(serde_json::from_value::<RemoteSendMessageRequest>(invalid).is_err());
    }
    for sender in [
        json!({"kind":"remote_api", "instance_id":InstanceId::new()}),
        json!({"kind":"system_generated"}),
        json!({"kind":"coordinator", "product_conversation_id":"source", "transcript_id":"source", "source_call":{"message_id":"half-call"}}),
        json!({"kind":"conversation", "product_conversation_id":"source", "transcript_id":"source", "source_call":{"message_id":"m", "tool_use_id":"t", "instance_id":InstanceId::new()}}),
    ] {
        let mut invalid = wire.clone();
        invalid["sender"] = sender;
        assert!(serde_json::from_value::<RemoteSendMessageRequest>(invalid).is_err());
    }
}

#[test]
fn literal_text_uuid_and_locator_budgets_are_enforced() {
    let wire = serde_json::to_value(request()).unwrap();
    for message in [
        "   \n".to_string(),
        "x".repeat(MAX_REMOTE_MESSAGE_BYTES + 1),
        "é".repeat(MAX_REMOTE_MESSAGE_BYTES / 2 + 1),
    ] {
        let mut invalid = wire.clone();
        invalid["message"] = json!(message);
        assert!(serde_json::from_value::<RemoteSendMessageRequest>(invalid).is_err());
    }
    let mut invalid = wire;
    invalid["message_id"] = json!("not-a-uuid");
    assert!(serde_json::from_value::<RemoteSendMessageRequest>(invalid).is_err());
    assert!(RemoteMessageText::parse("x".repeat(MAX_REMOTE_MESSAGE_BYTES)).is_ok());
    let mut oversized = request();
    oversized.target = RemoteSendTarget::Transcript {
        transcript_id: TranscriptConversationId::parse("x".repeat(1025)).unwrap(),
    };
    assert!(!oversized.locators_are_bounded());
    assert!(RemoteSendTarget::parse_tool_target("@transcript:id#message-citation").is_err());
    assert_eq!(
        RemoteSendTarget::parse_tool_target("@coordinator").unwrap(),
        RemoteSendTarget::Coordinator {}
    );
}

#[test]
fn attestation_binds_only_the_authenticated_source_instance() {
    let authenticated = InstanceId::new();
    for kind in ["conversation", "coordinator"] {
        let sender: RemoteSenderAttestation = serde_json::from_value(json!({
            "kind":kind,
            "product_conversation_id":"stable-source",
            "transcript_id":"exact-source",
            "source_call":{"message_id":"source-message", "tool_use_id":"source-call"},
        }))
        .unwrap();
        let origin = sender.into_origin(authenticated);
        let value = serde_json::to_value(&origin).unwrap();
        assert_eq!(value["instance_id"], json!(authenticated));
        assert_eq!(value["product_conversation_id"], "stable-source");
        assert_eq!(value["transcript_id"], "exact-source");
        assert_eq!(origin.source_call().unwrap().tool_use_id, "source-call");
        assert!(matches!(
            origin,
            InputOrigin::RemoteConversation { .. } | InputOrigin::RemoteCoordinator { .. }
        ));
    }
}

#[tokio::test]
async fn coordinator_sender_keeps_its_real_stable_identity_and_exact_member() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let id = coordinator.id.clone();
    let product = coordinator.product_conversation_id.clone();
    let sender = RemoteSenderAttestation::from_conversation(
        coordinator,
        SourceToolCall {
            message_id: "assistant".into(),
            tool_use_id: "send".into(),
        },
    )
    .unwrap();
    assert!(
        matches!(sender, RemoteSenderAttestation::Coordinator { product_conversation_id, transcript_id, source_call:Some(_) } if product_conversation_id == product && transcript_id.as_str() == id)
    );
}

fn accepted_response(request: &RemoteSendMessageRequest, caller: InstanceId) -> Value {
    serde_json::to_value(RemoteSendMessageResponse {
        destination_instance_id: request.destination_instance_id,
        caller_instance_id: caller,
        target: request.target.clone(),
        message_id: request.message_id,
        receipt: RemoteSendReceipt::Accepted {
            acceptance: RemoteMessageAcceptance {
                accepting_transcript: TranscriptConversationId::parse("recipient").unwrap(),
                stable_navigation: RemoteSendNavigation::AppLocal {
                    path: super::super::product_conversations::product_route(
                        None,
                        "stable-recipient",
                    ),
                },
                accepting_transcript_navigation: RemoteSendNavigation::AppLocal {
                    path: super::super::product_conversations::exact_transcript_route(
                        &TranscriptConversationId::parse("recipient").unwrap(),
                    ),
                },
                accepting_message_navigation: RemoteSendNavigation::AppLocal {
                    path: format!(
                        "{}{}",
                        super::super::product_conversations::exact_transcript_route(
                            &TranscriptConversationId::parse("recipient").unwrap(),
                        ),
                        super::super::product_conversations::message_fragment(&format!(
                            "recipient:{}",
                            request.message_id
                        )),
                    ),
                },
                stable_destination: RemoteStableDestination {
                    instance_id: request.destination_instance_id,
                    product_conversation_id: ProductConversationId::parse("stable-recipient")
                        .unwrap(),
                },
                disposition: RemoteSendDisposition::Delivered,
            },
        },
    })
    .unwrap()
}

#[test]
fn send_receipts_validate_identity_target_uuid_and_accepting_transcript() {
    let request = request();
    let caller = InstanceId::new();
    let valid = accepted_response(&request, caller);
    let decode = |value: &Value| {
        decode_remote_send_response(
            StatusCode::OK,
            &serde_json::to_vec(value).unwrap(),
            caller,
            &request,
        )
    };
    assert!(decode(&valid).is_ok());
    for (path, value) in [
        ("/destination_instance_id", json!(InstanceId::new())),
        ("/caller_instance_id", json!(InstanceId::new())),
        ("/message_id", json!(uuid::Uuid::new_v4())),
        ("/target/transcript_id", json!("other")),
        ("/receipt/acceptance/accepting_transcript", json!("other")),
        (
            "/receipt/acceptance/stable_destination/instance_id",
            json!(InstanceId::new()),
        ),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(path).unwrap() = value;
        assert!(decode(&invalid).is_err(), "accepted mismatched {path}");
    }
    let mut invalid = valid;
    invalid["receipt"]["acceptance"]["completed"] = json!(true);
    assert!(decode(&invalid).is_err());
    let mut stable_request = request.clone();
    stable_request.target = RemoteSendTarget::ProductConversation {
        product_conversation_id: ProductConversationId::parse("expected-stable").unwrap(),
    };
    let wrong_stable = accepted_response(&stable_request, caller);
    assert!(matches!(
        decode_remote_send_response(
            StatusCode::OK,
            &serde_json::to_vec(&wrong_stable).unwrap(),
            caller,
            &stable_request
        ),
        Err(FederationClientError::SendProvenanceMismatch)
    ));
}

#[test]
fn receipt_navigation_must_be_destination_paths_for_the_exact_acceptance() {
    let request = request();
    let caller = InstanceId::new();
    let valid = accepted_response(&request, caller);
    for (field, navigation) in [
        (
            "stable_navigation",
            json!({"kind":"peer", "url":"https://other.invalid/c/target"}),
        ),
        (
            "stable_navigation",
            json!({"kind":"app_local", "path":"https://user:secret@other.invalid/c/target"}),
        ),
        (
            "stable_navigation",
            json!({"kind":"app_local", "path":"/c/target?token=secret"}),
        ),
        (
            "stable_navigation",
            json!({"kind":"app_local", "path":"/c/target", "token":"secret"}),
        ),
        (
            "accepting_transcript_navigation",
            json!({"kind":"app_local", "path":"/c/other?source_transcript=other"}),
        ),
        (
            "accepting_message_navigation",
            json!({"kind":"app_local", "path":"/c/recipient?source_transcript=recipient#message-other"}),
        ),
    ] {
        let mut invalid = valid.clone();
        invalid["receipt"]["acceptance"][field] = navigation;
        assert!(
            decode_remote_send_response(
                StatusCode::OK,
                &serde_json::to_vec(&invalid).unwrap(),
                caller,
                &request
            )
            .is_err(),
            "accepted invalid {field}"
        );
    }
}

#[test]
fn empty_send_variants_are_closed_in_requests_and_responses() {
    for field in ["instance_id", "extra"] {
        let mut wire = serde_json::to_value(request()).unwrap();
        wire["sender"][field] = json!("untrusted");
        assert!(serde_json::from_value::<RemoteSendMessageRequest>(wire).is_err());
        let mut wire = accepted_response(&request(), InstanceId::new());
        wire["target"] = json!({"kind":"coordinator"});
        wire["target"][field] = json!("untrusted");
        assert!(serde_json::from_value::<RemoteSendMessageResponse>(wire).is_err());
    }
    assert!(serde_json::from_value::<RemoteSendTarget>(json!({"kind":"coordinator"})).is_ok());
    assert!(
        serde_json::from_value::<RemoteSenderAttestation>(json!({"kind":"remote_api"})).is_ok()
    );
}

#[test]
fn duplicate_and_rejected_receipts_remain_typed_and_do_not_claim_completion() {
    let request = request();
    let caller = InstanceId::new();
    let mut wire = accepted_response(&request, caller);
    wire["receipt"]["outcome"] = json!("duplicate");
    assert!(matches!(
        decode_remote_send_response(
            StatusCode::OK,
            &serde_json::to_vec(&wire).unwrap(),
            caller,
            &request
        )
        .unwrap()
        .receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    wire["receipt"] = json!({"outcome":"rejected", "code":"idempotency_conflict", "message":"different source instance"});
    assert!(matches!(
        decode_remote_send_response(
            StatusCode::OK,
            &serde_json::to_vec(&wire).unwrap(),
            caller,
            &request
        )
        .unwrap()
        .receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::IdempotencyConflict,
            ..
        }
    ));
    wire["receipt"]["code"] = json!("arbitrary_future_code");
    assert!(decode_remote_send_response(
        StatusCode::OK,
        &serde_json::to_vec(&wire).unwrap(),
        caller,
        &request
    )
    .is_err());
    assert!(
        decode_remote_send_response(StatusCode::TEMPORARY_REDIRECT, b"", caller, &request).is_err()
    );
}

#[test]
fn search_attests_foreign_sender_without_changing_primary_evidence_ownership() {
    use crate::api::global_read::{
        QualifiedSearchHit, QualifiedSearchNavigation, QualifiedSearchSender,
    };
    use phoenix_core::domain::conversation_reference::{MessageId, QualifiedConversationReference};
    let destination = InstanceId::new();
    let caller = InstanceId::new();
    let sender_instance = InstanceId::new();
    let transcript = TranscriptConversationId::parse("colliding-id").unwrap();
    let message_id = MessageId::parse("received-message").unwrap();
    let path = format!(
        "{}{}",
        crate::api::product_conversations::exact_transcript_route(&transcript),
        crate::api::product_conversations::message_fragment(message_id.as_str())
    );
    let response = RemoteSearchResponse {
        destination_instance_id: destination,
        caller_instance_id: caller,
        hits: vec![QualifiedSearchHit {
            source: QualifiedConversationReference::new(
                destination,
                ConversationReferenceTarget::Message {
                    transcript_id: transcript.clone(),
                    message_id,
                },
            ),
            source_navigation: Some(QualifiedSearchNavigation::AppLocal { path }),
            stable_conversation: None,
            stable_navigation: None,
            role: "Remote conversation".into(),
            sender: Some(QualifiedSearchSender {
                stable: QualifiedConversationReference::new(
                    sender_instance,
                    ConversationReferenceTarget::ProductConversation {
                        product_conversation_id: ProductConversationId::parse("colliding-id")
                            .unwrap(),
                    },
                ),
                exact: QualifiedConversationReference::new(
                    sender_instance,
                    ConversationReferenceTarget::Transcript {
                        transcript_id: transcript,
                    },
                ),
            }),
            created_at: chrono::Utc::now(),
            snippet: "received evidence".into(),
        }],
    };
    let wire = serde_json::to_value(response).unwrap();
    let decode = |wire: &Value| {
        decode_remote_search_response(
            StatusCode::OK,
            &serde_json::to_vec(wire).unwrap(),
            destination,
            caller,
        )
    };
    let accepted = decode(&wire).unwrap();
    let sender = accepted[0].sender.as_ref().unwrap();
    assert_eq!(sender.stable.instance_id, sender_instance);
    assert_eq!(sender.exact.instance_id, sender_instance);
    for (path, replacement) in [
        ("/hits/0/source/instance_id", json!(sender_instance)),
        ("/hits/0/sender/exact/instance_id", json!(destination)),
        (
            "/hits/0/sender/stable/target",
            json!({"kind":"transcript", "transcript_id":"colliding-id"}),
        ),
        ("/hits/0/sender/exact/target", json!({"kind":"coordinator"})),
        (
            "/hits/0/source_navigation",
            json!({"kind":"peer", "url":"https://other.invalid/"}),
        ),
    ] {
        let mut invalid = wire.clone();
        *invalid.pointer_mut(path).unwrap() = replacement;
        assert!(
            matches!(
                decode(&invalid),
                Err(FederationClientError::SearchProvenanceMismatch)
            ),
            "accepted {path}"
        );
    }
}
