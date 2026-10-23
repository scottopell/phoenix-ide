use super::*;
use sqlx::Row;

fn historical_projection(bytes: &[u8]) -> PreparedDirectTurnPayload {
    let raw: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        raw,
        serde_json::json!({
            "v": 2,
            "submitted": {
                "text": "Work on the admitted goal", "message_id": "goal-owner-input",
                "user_agent": null, "skill_invocation": null, "expansion_policy": "literal_text"
            },
            "delivery": {
                "text": "Work on the admitted goal", "llm_text": null,
                "user_agent": null, "skill_invocation": null
            }
        })
    );
    PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            text: raw["submitted"]["text"].as_str().unwrap().to_owned(),
            message_id: raw["submitted"]["message_id"].as_str().unwrap().to_owned(),
            origin: InputOrigin::UnknownHistorical,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: raw["delivery"]["text"].as_str().unwrap().to_owned(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    )
}

pub(super) async fn seed_materialized_message(
    db: &Database,
    execution: GoalExecution,
    expected: &PreparedDirectTurnPayload,
) {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let row = sqlx::query("SELECT turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,terminal_kind,canonical_message_id,owns_conversation,origin_kind FROM durable_turns WHERE turn_id=?")
        .bind(execution.turn_id()).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(row.get::<i64, _>("workflow_id"), execution.workflow_id());
    assert_eq!(row.get::<String, _>("disposition"), "Runtime");
    assert_eq!(row.get::<Option<String>, _>("terminal_kind"), None);
    assert_eq!(row.get::<Option<String>, _>("canonical_message_id"), None);
    assert_eq!(row.get::<i64, _>("owns_conversation"), 1);
    let columns: Vec<String> = sqlx::query("PRAGMA table_info(durable_turns)")
        .fetch_all(&mut *tx)
        .await
        .unwrap()
        .iter()
        .map(|column| column.get("name"))
        .collect();
    assert!(!columns
        .iter()
        .any(|name| name == "input_kind" || name == "materialized"));
    let conversation = ConversationAuthority(row.get("conversation_id"));
    let bytes: Vec<u8> = row.get("prepared_payload");
    assert_eq!(row.get::<String, _>("origin_kind"), "unknown_historical");
    let attachments: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM durable_turn_submitted_images WHERE turn_id=?1)+(SELECT count(*) FROM durable_turn_submitted_files WHERE turn_id=?1)+(SELECT count(*) FROM durable_turn_delivery_images WHERE turn_id=?1)+(SELECT count(*) FROM durable_turn_delivery_files WHERE turn_id=?1)")
        .bind(execution.turn_id()).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(attachments, 0);
    let payload = historical_projection(&bytes);
    assert_eq!(&payload, expected);
    assert_eq!(
        bytes,
        payload.to_normalized_bytes_without_attachments().unwrap()
    );
    let fingerprint: String = row.get("prepared_fingerprint");
    let prepared = PreparedTurn::rehydrate(
        &conversation,
        fingerprint.clone(),
        payload.to_exact_bytes().unwrap(),
    )
    .unwrap();
    let turn_id = TurnAuthorityId(u64::try_from(execution.turn_id()).unwrap());
    let generation = u64::try_from(row.get::<i64, _>("generation")).unwrap();
    let turn = phoenix_workflow::DurableTurn {
        id: turn_id,
        conversation: conversation.clone(),
        client_key: ClientTurnKey::new(row.get::<String, _>("client_turn_key")).unwrap(),
        prepared,
        generation,
        lifecycle: phoenix_workflow::TurnLifecycle::Accepted {
            disposition: AcceptedDisposition::Runtime,
        },
        materialization: phoenix_workflow::Materialization::Unmaterialized,
    };
    let message_id = format!("{}:{}", conversation.0, payload.submitted.message_id);
    let mut model = phoenix_workflow::DurableTurnModel::from_turns([turn]).unwrap();
    let step = model
        .apply(TurnCommand::Materialize {
            turn_id,
            expected_generation: generation,
            message_id: phoenix_workflow::CanonicalMessageId(message_id.clone()),
        })
        .unwrap();
    assert!(matches!(step.outcome, TurnOutcome::Materialized { .. }));
    let (content, display) = payload.message_content_and_display_data();
    sqlx::query("INSERT INTO messages(message_id,conversation_id,sequence_id,message_type,content,display_data,created_at,origin_kind) VALUES (?,?,2,'user',?,?,'2026-10-19T00:00:00Z','unknown_historical')")
        .bind(&message_id).bind(&conversation.0)
        .bind(serde_json::to_string(&content.to_stored_json()).unwrap())
        .bind(display.map(|value| serde_json::to_string(&value).unwrap()))
        .execute(&mut *tx).await.unwrap();
    let updated = sqlx::query("UPDATE durable_turns SET canonical_message_id=? WHERE turn_id=? AND workflow_id=? AND generation=? AND terminal_kind IS NULL AND canonical_message_id IS NULL")
        .bind(&message_id).bind(execution.turn_id()).bind(execution.workflow_id()).bind(i64::try_from(generation).unwrap())
        .execute(&mut *tx).await.unwrap().rows_affected();
    assert_eq!(updated, 1);
    let state = ConvState::LlmRequesting { attempt: 1 };
    sqlx::query("UPDATE conversations SET state=?,state_kind=?,state_updated_at='2026-10-19T00:00:00Z',updated_at='2026-10-19T00:00:00Z' WHERE id=?")
        .bind(serde_json::to_string(&state).unwrap()).bind(crate::conv_state_kind(&state)).bind(&conversation.0)
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let stored: (String,Vec<u8>,i64,String) = sqlx::query_as("SELECT canonical_message_id,prepared_payload,generation,prepared_fingerprint FROM durable_turns WHERE turn_id=?")
        .bind(execution.turn_id()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        stored,
        ("full-root:goal-owner-input".into(), bytes, 0, fingerprint)
    );
    assert!(model.invariant_violations().is_empty());
}
