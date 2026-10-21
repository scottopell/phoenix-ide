use super::*;
use phoenix_core::domain::sm_state::{
    ServerOverloadPhase, ServerOverloadRetry, ServerOverloadTarget,
};
use phoenix_workflow::direct_turn_profile;

async fn materialized() -> (Database, GoalStartAdoptionInput) {
    let (db, input) = claimed().await;
    let result = db
        .workflow_repository()
        .materialize_goal_start(&input)
        .await;
    assert!(
        matches!(
            &result,
            LocalAuthorityResult::DurableFactEstablished(
                GoalStartAdoptionOutcome::Committed { .. }
            )
        ),
        "expected committed Start materialization, got {result:?}"
    );
    (db, input)
}

async fn set_retry(
    db: &Database,
    input: &GoalStartAdoptionInput,
    target: ServerOverloadTarget,
    phase: ServerOverloadPhase,
    request_id: String,
) {
    let state = ConvState::ServerOverloadRetrying {
        retry: ServerOverloadRetry {
            target,
            phase,
            attempt: 1,
            started_at: input.state_updated_at,
            deadline_at: input.state_updated_at + chrono::Duration::seconds(60),
            logical_request_id: request_id,
            model_id: "test-model".into(),
        },
    };
    sqlx::query("UPDATE conversations SET state=?,state_kind=? WHERE id=?")
        .bind(serde_json::to_string(&state).unwrap())
        .bind(crate::conv_state_kind(&state))
        .bind(input.receipt.historical_transcript().as_str())
        .execute(db.pool())
        .await
        .unwrap();
}

async fn context(
    db: &Database,
    input: &GoalStartAdoptionInput,
) -> crate::DbResult<Option<crate::workflow::GoalStartRequestContext>> {
    db.workflow_repository()
        .admit_goal_start_request_context(
            input.receipt.historical_transcript().as_str(),
            Some((
                TurnAuthorityId(u64::try_from(input.receipt.execution().turn_id()).unwrap()),
                input.authority.generation.0,
            )),
            phoenix_core::domain::conversation_goal::GoalRequestOperation::Normal { attempt: 1 },
        )
        .await
}

#[tokio::test]
async fn conversation_goal_start_retry_context_restores_same_id_ordinary_inflight_request() {
    let (db, input) = materialized().await;
    let before = materialization_counts(&db).await;
    set_retry(
        &db,
        &input,
        ServerOverloadTarget::Ordinary,
        ServerOverloadPhase::InFlight,
        input.intent().unwrap().id.as_str().into(),
    )
    .await;
    let restored = context(&db, &input).await.unwrap().unwrap();
    assert_eq!(restored.request_id, input.intent().unwrap().id);
    assert_eq!(&restored.observation, input.receipt.observation());
    assert_eq!(materialization_counts(&db).await, before);
}

#[tokio::test]
async fn conversation_goal_start_retry_context_rejects_different_logical_request_id() {
    let (db, input) = materialized().await;
    set_retry(
        &db,
        &input,
        ServerOverloadTarget::Ordinary,
        ServerOverloadPhase::InFlight,
        "different-request".into(),
    )
    .await;
    assert!(context(&db, &input).await.is_err());
}

#[tokio::test]
async fn conversation_goal_start_retry_context_rejects_continuation_target_with_same_id() {
    let (db, input) = materialized().await;
    set_retry(
        &db,
        &input,
        ServerOverloadTarget::Continuation {
            operation_id: "continuation".into(),
            rejected_tool_calls: vec![],
        },
        ServerOverloadPhase::InFlight,
        input.intent().unwrap().id.as_str().into(),
    )
    .await;
    assert!(context(&db, &input).await.is_err());
}

#[tokio::test]
async fn conversation_goal_start_retry_context_rejects_waiting_request_before_dispatch() {
    let (db, input) = materialized().await;
    set_retry(
        &db,
        &input,
        ServerOverloadTarget::Ordinary,
        ServerOverloadPhase::Waiting {
            retry_at: input.state_updated_at + chrono::Duration::seconds(5),
        },
        input.intent().unwrap().id.as_str().into(),
    )
    .await;
    assert!(context(&db, &input).await.is_err());
}

#[tokio::test]
async fn conversation_goal_start_committed_probe_rejects_transition_generation_divergence() {
    let (db, input) = materialized().await;
    sqlx::query("UPDATE workflow_transitions SET generation=generation+1 WHERE workflow_id=? AND transition_id=2")
        .bind(input.receipt.execution().workflow_id()).execute(db.pool()).await.unwrap();
    assert!(db
        .workflow_repository()
        .classify_goal_start_materialization(&input)
        .await
        .is_err());
}

#[tokio::test]
async fn conversation_goal_start_committed_probe_does_not_adopt_divergent_workflow_head() {
    for mutation in [
        "UPDATE workflows SET snapshot_payload=X'7B7D' WHERE workflow_id=?",
        "UPDATE workflows SET snapshot_codec_version=snapshot_codec_version+1 WHERE workflow_id=?",
        "UPDATE workflows SET status='Cancelling' WHERE workflow_id=?",
        "UPDATE workflows SET version=version-1 WHERE workflow_id=?",
    ] {
        let (db, input) = materialized().await;
        sqlx::query(mutation)
            .bind(input.receipt.execution().workflow_id())
            .execute(db.pool())
            .await
            .unwrap();
        assert!(
            db.workflow_repository()
                .classify_goal_start_materialization(&input)
                .await
                .is_err(),
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn conversation_goal_start_committed_probe_classifies_valid_advanced_head_elsewhere() {
    let (db, input) = materialized().await;
    sqlx::query("UPDATE workflows SET version=version+1 WHERE workflow_id=?")
        .bind(input.receipt.execution().workflow_id())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.workflow_repository()
            .classify_goal_start_materialization(&input)
            .await
            .unwrap(),
        GoalStartAdoptionOutcome::CommittedElsewhere
    );
}

async fn partial_receipt(db: &Database, input: &GoalStartAdoptionInput) {
    let canonical = direct_turn_profile::CanonicalTurnReceipt::GoalStart(
        direct_turn_profile::GoalStartTurnReceipt {
            turn_id: u64::try_from(input.receipt.execution().turn_id()).unwrap(),
            goal_start_receipt_id: input.start_id,
        },
    );
    sqlx::query("INSERT INTO workflow_receipts(workflow_id,receipt_id,effect_id,generation,declared_workflow_version,process_incarnation,attempt_id,origin,receipt_codec_family,receipt_codec_version,receipt_payload) VALUES (?,1,?,?,?,?,?,'Execution',?,?,?)")
        .bind(input.receipt.execution().workflow_id())
        .bind(i64::try_from(input.authority.effect_id.0).unwrap()).bind(input.intent().unwrap().generation())
        .bind(i64::try_from(input.authority.declared_workflow_version.0).unwrap())
        .bind(i64::try_from(input.authority.process_incarnation.0).unwrap())
        .bind(i64::try_from(input.authority.attempt_id.0).unwrap())
        .bind(direct_turn_profile::RECEIPT_CODEC_FAMILY).bind(i64::from(direct_turn_profile::PROTOCOL_VERSION))
        .bind(serde_json::to_vec(&canonical).unwrap()).execute(db.pool()).await.unwrap();
}

async fn partial_delivery(db: &Database, input: &GoalStartAdoptionInput) {
    let event = direct_turn_profile::DirectTurnReceiptEvent::GoalStartMaterialized {
        receipt: input.start_id,
    };
    sqlx::query("INSERT INTO workflow_deliveries(workflow_id,delivery_id,effect_id,consumer_kind,event_codec_family,event_codec_version,payload_kind,payload_blob,requires_runtime_acceptance,status,runtime_acceptance_status) VALUES (?,1,?,'reducer',?,?,'Receipt',?,1,'Pending','Owed')")
        .bind(input.receipt.execution().workflow_id()).bind(i64::try_from(input.authority.effect_id.0).unwrap())
        .bind(direct_turn_profile::RECEIPT_EVENT_CODEC_FAMILY).bind(i64::from(direct_turn_profile::PROTOCOL_VERSION))
        .bind(serde_json::to_vec(&event).unwrap()).execute(db.pool()).await.unwrap();
}

async fn assert_partial_unclassified(db: &Database, input: &GoalStartAdoptionInput) {
    let repo = db.workflow_repository();
    assert!(repo
        .classify_goal_start_materialization(input)
        .await
        .is_err());
    sqlx::raw_sql("CREATE TRIGGER fail_review_materialization BEFORE UPDATE OF materialized ON durable_turns BEGIN SELECT RAISE(ABORT,'review injected materialization failure'); END;")
        .execute(db.pool()).await.unwrap();
    assert!(matches!(
        repo.materialize_goal_start(input).await,
        LocalAuthorityResult::DurableFactUnclassified
    ));
}

#[tokio::test]
async fn conversation_goal_start_uncommitted_probe_rejects_partial_receipt() {
    let (db, input) = claimed().await;
    partial_receipt(&db, &input).await;
    assert_partial_unclassified(&db, &input).await;
}

#[tokio::test]
async fn conversation_goal_start_uncommitted_probe_rejects_partial_delivery() {
    let (db, input) = claimed().await;
    partial_delivery(&db, &input).await;
    assert_partial_unclassified(&db, &input).await;
}

#[tokio::test]
async fn conversation_goal_start_uncommitted_probe_requires_exact_prematerialization_head() {
    for mutation in [
        "UPDATE workflows SET snapshot_payload=X'7B7D' WHERE workflow_id=?",
        "UPDATE workflows SET snapshot_codec_family='different.snapshot' WHERE workflow_id=?",
        "UPDATE workflows SET status='Cancelling' WHERE workflow_id=?",
        "UPDATE workflows SET version=version+1 WHERE workflow_id=?",
    ] {
        let (db, input) = claimed().await;
        sqlx::query(mutation)
            .bind(input.receipt.execution().workflow_id())
            .execute(db.pool())
            .await
            .unwrap();
        assert!(
            db.workflow_repository()
                .classify_goal_start_materialization(&input)
                .await
                .is_err(),
            "{mutation}"
        );
    }
}

async fn raw_witness(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &GoalStartAdoptionInput,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    sqlx::query("INSERT INTO conversation_goal_start_materializations(start_id,request_id,effect_id,attempt_id,process_incarnation,state_updated_at) VALUES (?,?,?,?,?,?)")
        .bind(i64::try_from(input.start_id.get()).unwrap()).bind(input.intent().unwrap().id.as_str())
        .bind(i64::try_from(input.authority.effect_id.0).unwrap()).bind(i64::try_from(input.authority.attempt_id.0).unwrap())
        .bind(i64::try_from(input.authority.process_incarnation.0).unwrap()).bind(input.state_updated_at.to_rfc3339())
        .execute(&mut **tx).await
}

async fn remove_witness_for_forgery_test(db: &Database) -> sqlx::Transaction<'_, sqlx::Sqlite> {
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("DROP TRIGGER conversation_goal_start_materializations_no_delete")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM conversation_goal_start_materializations")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

#[tokio::test]
async fn conversation_goal_start_witness_rejects_staged_joined_subset_and_one_field_forgery() {
    for mutation in [
        "DELETE FROM workflow_deliveries WHERE workflow_id=?",
        "UPDATE workflow_deliveries SET status='Pending',runtime_acceptance_status='Owed',accepted_by_transition_id=NULL WHERE workflow_id=?",
        "UPDATE workflow_deliveries SET payload_blob=X'7B7D' WHERE workflow_id=?",
        "UPDATE workflow_deliveries SET event_codec_version=event_codec_version+1 WHERE workflow_id=?",
        "UPDATE workflow_receipts SET receipt_payload=X'7B7D' WHERE workflow_id=?",
        "UPDATE workflow_receipts SET receipt_codec_family='different.receipt' WHERE workflow_id=?",
        "UPDATE workflow_transitions SET event_payload=X'7B7D' WHERE workflow_id=? AND transition_id=2",
        "UPDATE workflow_transitions SET event_codec_version=event_codec_version+1 WHERE workflow_id=? AND transition_id=2",
        "UPDATE workflows SET snapshot_payload=X'7B7D' WHERE workflow_id=?",
        "UPDATE workflows SET status='Cancelling' WHERE workflow_id=?",
    ] {
        let (db,input)=materialized().await;
        let mut tx=remove_witness_for_forgery_test(&db).await;
        sqlx::query(mutation).bind(input.receipt.execution().workflow_id()).execute(&mut *tx).await.unwrap();
        assert!(raw_witness(&mut tx,&input).await.is_err(),"{mutation}");
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
async fn conversation_goal_start_witness_requires_exact_initial_state_and_timestamp() {
    for state in [ConvState::LlmRequesting { attempt: 2 }, ConvState::Idle] {
        let (db, input) = materialized().await;
        let mut tx = remove_witness_for_forgery_test(&db).await;
        sqlx::query("UPDATE conversations SET state=?,state_kind=? WHERE id=?")
            .bind(serde_json::to_string(&state).unwrap())
            .bind(crate::conv_state_kind(&state))
            .bind(input.receipt.historical_transcript().as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(raw_witness(&mut tx, &input).await.is_err());
        tx.rollback().await.unwrap();
    }
    let (db, input) = materialized().await;
    let mut tx = remove_witness_for_forgery_test(&db).await;
    sqlx::query("UPDATE conversations SET state_updated_at=? WHERE id=?")
        .bind((input.state_updated_at + chrono::Duration::nanoseconds(1)).to_rfc3339())
        .bind(input.receipt.historical_transcript().as_str())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(raw_witness(&mut tx, &input).await.is_err());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn conversation_goal_start_witness_complete_canonical_bundle_passes_raw_guard() {
    let (db, input) = materialized().await;
    let mut tx = remove_witness_for_forgery_test(&db).await;
    raw_witness(&mut tx, &input).await.unwrap();
    tx.rollback().await.unwrap();
}
