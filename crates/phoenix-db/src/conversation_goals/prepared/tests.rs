use super::*;
use crate::conversation_goals::user_commands::{GoalUserRefusal, GoalUserResult};
use crate::conversation_goals::writer_tests::{at, counts, fixture, ordinary};
use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_event::{
    DirectTurnTurnId, PreparedDirectTurnDelivery, PreparedDirectTurnPayload,
    SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
};

fn runtime_input(transcript: &str, key: &str) -> crate::workflow::AcceptAuthoritativeTurn {
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: key.into(),
            origin: InputOrigin::UserApi,
            text: "source input".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "source input".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    crate::workflow::AcceptAuthoritativeTurn {
        client_key: ClientTurnKey::new(key).unwrap(),
        prepared: PreparedTurn::from_exact_payload(
            &ConversationAuthority(transcript.into()),
            payload.to_exact_bytes().unwrap(),
        ),
        disposition: AcceptedDisposition::Runtime,
        accepted_at: Timestamp(1),
    }
}

async fn coordinator_source(
    db: &Database,
    key: &str,
) -> (
    String,
    ProductConversationId,
    SourceToolCall,
    DirectTurnTurnId,
) {
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let caller = coordinator.id;
    let owner = coordinator.product_conversation_id;
    let accepted = db
        .workflow_repository()
        .accept_authoritative_turn(&runtime_input(&caller, key))
        .await
        .unwrap();
    let phoenix_workflow::TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("expected fresh coordinator Runtime turn");
    };
    let source_call = SourceToolCall {
        message_id: format!("{key}-agent"),
        tool_use_id: format!("{key}-call"),
    };
    db.add_message(
        &source_call.message_id,
        &caller,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: source_call.tool_use_id.clone(),
            name: "goal_create".into(),
            input: serde_json::json!({"objective": "from coordinator"}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    (caller, owner, source_call, DirectTurnTurnId(turn_id.0))
}

fn create_intent(objective: &str) -> GoalPreparationIntent {
    GoalPreparationIntent::Create {
        objective: GoalText::parse(objective).unwrap(),
    }
}

async fn preparation_count(db: &Database) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_preparations")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn goal_preparation_migration_follows_conversation_tool_migrations() {
    let db = fixture().await;
    let migrations: Vec<(i64, String)> =
        sqlx::query_as("SELECT version,name FROM _migrations WHERE version>=143 ORDER BY version")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        migrations,
        vec![
            (143, "agent_creation_provenance".into()),
            (144, "product_conversation_rename_receipts".into()),
            (145, "coordinator_goal_preparations".into()),
            (146, "retain_product_conversation_rename_receipts".into()),
            (147, "retain_product_creation_receipts".into()),
            (148, "product_creation_initial_execution_receipts".into()),
            (149, "product_creation_image_retirement".into())
        ]
    );
}

#[tokio::test]
async fn prepare_retains_exact_source_and_payload_without_mutating_or_dispatching() {
    let db = fixture().await;
    let (caller, source_product, source_call, turn) = coordinator_source(&db, "prep-retry").await;
    let (transcript, owner) = ordinary(&db, "prep-retry-owner").await;
    let before = counts(&db).await;
    let turns_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("first objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    assert_eq!(action.owner, owner);
    assert_eq!(action.expected_version, GoalVersion::INITIAL);
    assert_eq!(action.observed_goal_id, None);
    assert_eq!(action.source.product_conversation_id, source_product);
    assert_eq!(action.source.transcript_id.as_str(), caller);
    assert_eq!(
        action.source.execution.turn_id(),
        i64::try_from(turn.0).unwrap()
    );
    assert_eq!(action.source.message_id.as_str(), source_call.message_id);
    assert_eq!(action.source.tool_use_id.as_str(), source_call.tool_use_id);
    assert_eq!(preparation_count(&db).await, 1);
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM durable_turns")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        turns_before
    );
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());

    let intervention = crate::conversation_goals::user_commands::GoalUserCommand {
        owner: owner.clone(),
        principal_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: crate::conversation_goals::user_commands::GoalUserChange::Create {
            objective: GoalText::parse("intervening objective").unwrap(),
        },
    };
    db.apply_user_goal_command(
        &intervention,
        &transcript,
        || Ok(at()),
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let after = counts(&db).await;
    let retry = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("first objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    assert_eq!(retry.reference, action.reference);
    assert_eq!(retry.expected_version, action.expected_version);
    assert_eq!(retry.intent, action.intent);
    let fetched = db
        .read_coordinator_goal_preparation(&caller, action.reference)
        .await
        .unwrap();
    assert_eq!(fetched.reference, action.reference);
    assert_eq!(fetched.source, action.source);
    assert_eq!(counts(&db).await, after);
    assert_eq!(preparation_count(&db).await, 1);
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("changed objective")
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(preparation_count(&db).await, 1);
}

#[tokio::test]
async fn preparation_acknowledged_rollback_is_established_and_preserves_retained_action() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-rollback").await;
    let (_, owner) = ordinary(&db, "prep-rollback-owner").await;
    let retained = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("original"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    let before = counts(&db).await;
    let conflict = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("changed"),
        )
        .await;
    assert!(matches!(
        conflict,
        crate::workflow::LocalAuthorityResult::Established(Err(GoalDbError::RetryConflict))
    ));
    assert_eq!(preparation_count(&db).await, 1);
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.prepared_goal_action(&retained.reference)
            .await
            .unwrap()
            .unwrap(),
        retained
    );
}

#[tokio::test]
async fn preparation_lost_commit_ack_is_unclassified_and_source_retry_recovers_one_action() {
    let db = fixture().await;
    let before = counts(&db).await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-lost-ack").await;
    let (_, owner) = ordinary(&db, "prep-lost-ack-owner").await;
    let intent = create_intent("retained after lost commit acknowledgment");

    let result = db
        .prepare_coordinator_goal_action_with_commit(
            &caller,
            &owner,
            &source_call,
            turn,
            intent.clone(),
            PreparationCommit::LoseAcknowledgement,
        )
        .await;
    assert!(matches!(
        result,
        crate::workflow::LocalAuthorityResult::DurableFactUnclassified
    ));
    assert_eq!(preparation_count(&db).await, 1);
    assert_eq!(counts(&db).await, before);
    let retained_row = sqlx::query("SELECT * FROM conversation_goal_preparations")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let retained = decode(&retained_row).unwrap();

    let recovered = db
        .prepare_coordinator_goal_action(&caller, &owner, &source_call, turn, intent)
        .await
        .established()
        .expect("source retry commit was acknowledged")
        .unwrap();
    assert_eq!(recovered, retained);
    assert_eq!(preparation_count(&db).await, 1);
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.read_coordinator_goal_preparation(&caller, recovered.reference)
            .await
            .unwrap(),
        recovered
    );
}

#[tokio::test]
async fn prepared_command_observes_version_cas_and_replays_retained_refusal() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-cas").await;
    let (transcript, owner) = ordinary(&db, "prep-cas-owner").await;
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("stale objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    let command = action.command(Uuid::new_v4()).unwrap();
    let intervention = crate::conversation_goals::user_commands::GoalUserCommand {
        owner: owner.clone(),
        principal_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: crate::conversation_goals::user_commands::GoalUserChange::Create {
            objective: GoalText::parse("newer objective").unwrap(),
        },
    };
    db.apply_user_goal_command(
        &intervention,
        &transcript,
        || Ok(at()),
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let receipt = db
        .apply_coordinator_goal_command(
            &caller,
            &action.source,
            &command,
            &transcript,
            false,
            || Ok(at()),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(
        receipt.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Version,
            ..
        }
    ));
    let before = counts(&db).await;
    let retry = db
        .apply_coordinator_goal_command(
            &caller,
            &action.source,
            &command,
            &transcript,
            false,
            || panic!("retained refusal must not re-clock"),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(retry, receipt);
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.conversation_goal(&owner)
            .await
            .unwrap()
            .unwrap()
            .objective()
            .as_str(),
        "newer objective"
    );
}

#[tokio::test]
async fn prepared_command_accepts_real_coordinator_source_and_replays_without_mutation() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-accepted").await;
    let (transcript, owner) = ordinary(&db, "prep-accepted-owner").await;
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("coordinator objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    let command = action.command(Uuid::new_v4()).unwrap();
    assert_eq!(command.request_id, action.reference);
    let receipt = db
        .apply_coordinator_goal_command(
            &caller,
            &action.source,
            &command,
            &transcript,
            false,
            || Ok(at()),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(receipt.result, GoalUserResult::Accepted { .. }));
    let history = db
        .conversation_goal_history(&owner, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert!(matches!(&history[0].mutation().attribution.requested_by,
        phoenix_core::domain::conversation_goal::GoalActor::Coordinator(source) if source == &action.source));
    let before = counts(&db).await;
    let retry = db
        .apply_coordinator_goal_command(
            &caller,
            &action.source,
            &command,
            &transcript,
            false,
            || panic!("accepted replay must not re-clock"),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(retry, receipt);
    assert_eq!(counts(&db).await, before);
}

#[tokio::test]
async fn preparation_requires_exact_server_turn_and_current_coordinator() {
    let db = fixture().await;
    let (ordinary_id, owner) = ordinary(&db, "prepared-authority-target").await;
    let (caller, _, call, turn) = coordinator_source(&db, "prepared-authority").await;
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            &caller,
            &owner,
            &call,
            DirectTurnTurnId(0),
            create_intent("wrong execution"),
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::TargetChanged)
    ));
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            &ordinary_id,
            &owner,
            &call,
            turn,
            create_intent("wrong role"),
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::TargetChanged)
    ));
    assert_eq!(preparation_count(&db).await, 0);
    let prepared = db
        .prepare_coordinator_goal_action(&caller, &owner, &call, turn, create_intent("retained"))
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    assert!(matches!(
        db.prepare_coordinator_goal_action(
            &caller,
            &owner,
            &call,
            DirectTurnTurnId(0),
            create_intent("retained"),
        )
        .await
        .established()
        .expect("preparation authority classified"),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(
        db.read_coordinator_goal_preparation(&caller, prepared.reference)
            .await
            .unwrap()
            .expected_version,
        GoalVersion::INITIAL
    );
}

#[tokio::test]
async fn coordinator_payload_and_source_are_checked_before_busy_and_receipt_replay() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "prep-poison").await;
    let (transcript, owner) = ordinary(&db, "prep-poison-owner").await;
    let action = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("authorized objective"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    let command = action.command(Uuid::new_v4()).unwrap();

    let mut poisoned = action.command(command.principal_id).unwrap();
    poisoned.change = crate::conversation_goals::user_commands::GoalUserChange::Create {
        objective: GoalText::parse("poison objective").unwrap(),
    };
    assert!(matches!(
        db.apply_coordinator_goal_command(
            &caller,
            &action.source,
            &poisoned,
            &transcript,
            true,
            || Ok(at()),
            || panic!("unexpected authority loss")
        )
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::RetryConflict)
    ));
    let mut wrong_source = action.source.clone();
    wrong_source.tool_use_id = GoalText::parse("other-tool-use").unwrap();
    assert!(matches!(
        db.apply_coordinator_goal_command(
            &caller,
            &wrong_source,
            &command,
            &transcript,
            true,
            || Ok(at()),
            || panic!("unexpected authority loss")
        )
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_coordinator_receipts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );

    let receipt = db
        .apply_coordinator_goal_command(
            &caller,
            &action.source,
            &command,
            &transcript,
            false,
            || Ok(at()),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(receipt.result, GoalUserResult::Accepted { .. }));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_goal_coordinator_receipts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        db.apply_coordinator_goal_command(
            &caller,
            &action.source,
            &poisoned,
            &transcript,
            false,
            || panic!("poisoned replay must not clock"),
            || panic!("unexpected authority loss")
        )
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(
        db.retained_coordinator_goal_command(&caller, &command)
            .await
            .unwrap(),
        Some(receipt)
    );
}

#[tokio::test]
async fn preparation_schema_rejects_update_delete_replace_and_malformed_shape() {
    let db = fixture().await;
    let (_, owner) = ordinary(&db, "prepared-schema-target").await;
    let (caller, _, call, turn) = coordinator_source(&db, "prepared-schema").await;
    let prepared = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &call,
            turn,
            create_intent("immutable action"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    assert!(sqlx::query(
        "UPDATE conversation_goal_preparations SET objective='overwrite' WHERE reference=?"
    )
    .bind(prepared.reference.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM conversation_goal_preparations WHERE reference=?")
            .bind(prepared.reference.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query("INSERT OR REPLACE INTO conversation_goal_preparations SELECT * FROM conversation_goal_preparations WHERE reference=?")
        .bind(prepared.reference.as_bytes().as_slice()).execute(db.pool()).await.is_err());
    assert!(sqlx::query("INSERT INTO conversation_goal_preparations (reference,owner_id,caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,operation,expected_version,observed_goal_id,objective,disposition,reason) SELECT ?,owner_id,caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,'malformed-shape','create',1,NULL,objective,NULL,NULL FROM conversation_goal_preparations WHERE reference=?")
        .bind(Uuid::new_v4().as_bytes().as_slice()).bind(prepared.reference.as_bytes().as_slice()).execute(db.pool()).await.is_err());
    assert_eq!(preparation_count(&db).await, 1);
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
}

#[tokio::test]
async fn preparation_retains_source_locators_after_message_deletion_and_owner_cascade() {
    let db = fixture().await;
    let (caller, _, source_call, turn) = coordinator_source(&db, "retained-source").await;
    let (transcript, owner) = ordinary(&db, "retained-source-owner").await;
    let prepared = db
        .prepare_coordinator_goal_action(
            &caller,
            &owner,
            &source_call,
            turn,
            create_intent("retained source"),
        )
        .await
        .established()
        .expect("preparation authority classified")
        .unwrap();
    let command = prepared.command(Uuid::new_v4()).unwrap();
    db.apply_coordinator_goal_command(
        &caller,
        &prepared.source,
        &command,
        &transcript,
        false,
        || Ok(at()),
        || panic!("unexpected source failure"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    sqlx::query("DELETE FROM messages WHERE message_id=?")
        .bind(&source_call.message_id)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.read_coordinator_goal_preparation(&caller, prepared.reference)
            .await
            .unwrap(),
        prepared
    );
    let replay = db
        .retained_coordinator_goal_command(&caller, &command)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(replay.result, GoalUserResult::Accepted { .. }));
    db.delete_conversation(&transcript).await.unwrap();
    assert_eq!(preparation_count(&db).await, 0);
}
