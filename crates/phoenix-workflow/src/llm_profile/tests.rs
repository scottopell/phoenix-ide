use crate::llm_profile::*;
use crate::*;

fn sample_request() -> PreparedLlmRequest {
    PreparedLlmRequest {
        codec_version: PROTOCOL_VERSION,
        request_fingerprint: "req-1".into(),
        provider: "openai".into(),
        model: "gpt-5".into(),
        backend: "responses".into(),
        request_aggregate: "{\"messages\":[]}".into(),
    }
}

fn sample_response() -> CompleteLlmResponse {
    CompleteLlmResponse {
        codec_version: PROTOCOL_VERSION,
        response_fingerprint: "resp-1".into(),
        response_aggregate: "{\"output\":[]}".into(),
    }
}

fn sample_snapshot() -> TopLevelLlmSnapshot {
    TopLevelLlmSnapshot {
        turn_ref: TopLevelTurnRef {
            conversation_id: "conv-1".into(),
            accepted_turn_id: "turn-1".into(),
            generation: 4,
        },
        key: LlmEffectKey {
            accepted_turn_id: "turn-1".into(),
            generation: 4,
            call_ordinal: 2,
        },
        prepared_request: sample_request(),
        status: LlmEffectStatus::Prepared,
        accepted_assistant_message_id: None,
        stopped_at: None,
    }
}

fn sample_receipt() -> LlmResponseReceipt {
    LlmResponseReceipt {
        key: LlmEffectKey {
            accepted_turn_id: "turn-1".into(),
            generation: 4,
            call_ordinal: 2,
        },
        response: sample_response(),
        generation: 4,
    }
}

#[test]
fn acceptance_profile_exposes_llm_codec_support() {
    let acceptance = acceptance_profile();
    assert!(acceptance.supported_codecs.supports(&snapshot_codec()));
    assert!(acceptance.supported_codecs.supports(&event_codec()));
    assert!(acceptance.supported_codecs.supports(&intent_codec()));
    assert!(acceptance.supported_codecs.supports(&receipt_codec()));
    assert!(acceptance.runtime_acceptance_enabled());
    assert!(!acceptance.external_acceptance_enabled());
}

#[test]
fn prepared_plan_declares_safely_repeatable_llm_effect() {
    let snapshot = sample_snapshot();
    let intent = LlmIntent {
        key: snapshot.key.clone(),
        prepared_request: snapshot.prepared_request.clone(),
    };
    let plan = prepared_plan(snapshot.clone(), TopLevelLlmEvent::Prepared, intent.clone());

    assert_eq!(plan.snapshot, snapshot);
    assert_eq!(plan.effects.len(), 1);
    assert_eq!(plan.effects[0].effect_id, EFFECT_ID);
    assert_eq!(plan.effects[0].capability, ExecutionCapability::SafelyRepeatable);
    assert_eq!(plan.effects[0].intent, intent);
}

#[test]
fn receipt_delivery_and_runtime_mapping_matches_events() {
    let receipt = sample_receipt();
    let item = DeliveryItem::<LlmProfile> {
        id: DeliveryId(1),
        effect_id: Some(EFFECT_ID),
        barrier_id: None,
        consumer_kind: CONSUMER_KIND_TOP_LEVEL_CONVERSATION,
        event_codec: receipt_codec(),
        requires_runtime_acceptance: true,
        payload: DeliveryPayload::Receipt(receipt.clone()),
        status: DeliveryStatus::Pending,
        runtime_acceptance_status: None,
        suppression_reason: None,
        accepted_by: None,
    };

    assert!(LlmProfile::decision_handles_delivery(
        &item,
        &TopLevelLlmEvent::ResponseReceipted {
            response: Box::new(receipt.response.clone()),
            generation: receipt.generation,
        }
    ));
    assert!(LlmProfile::decision_handles_runtime_acceptance(
        &item,
        &TopLevelLlmEvent::ResponseAccepted {
            response: Box::new(receipt.response.clone()),
            assistant_message_id: "msg-1".into(),
        }
    ));
    assert!(LlmProfile::decision_handles_runtime_suppression(
        &item,
        &TopLevelLlmEvent::ResponseCancelled {
            response: Box::new(receipt.response.clone()),
            reason: "stop won".into(),
        }
    ));
    assert!(LlmProfile::receipt_requires_runtime_acceptance(&receipt));
}

#[test]
fn owned_profile_payloads_round_trip_with_serde() {
    let snapshot = sample_snapshot();
    let receipt = sample_receipt();

    let snapshot_json = serde_json::to_string(&snapshot).expect("serialize snapshot");
    let receipt_json = serde_json::to_string(&receipt).expect("serialize receipt");

    assert_eq!(
        serde_json::from_str::<TopLevelLlmSnapshot>(&snapshot_json).expect("deserialize snapshot"),
        snapshot
    );
    assert_eq!(
        serde_json::from_str::<LlmResponseReceipt>(&receipt_json).expect("deserialize receipt"),
        receipt
    );
}
