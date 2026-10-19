use super::*;

async fn fixture() -> (Database, GoalMutation) {
    let (db, owner) = full_substrate().await;
    let mut command = create();
    command.owner = owner;
    command.attribution.requested_by = GoalActor::UserApi {
        transcript_id: t("full-root"),
        message_id: t("full-input"),
    };
    (db, command)
}

async fn live_turn(db: &Database) {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query("INSERT INTO workflows (workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'direct_turn',1,1,1,0,0,'Active','direct_turn',1,X'00',0,0)")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO durable_turns (turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation) VALUES (1,1,'full-root','full-turn','fixture',X'00','Runtime',0,1)")
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

async fn assert_busy(db: &Database, command: &GoalMutation) {
    assert!(matches!(
        db.apply_conversation_goal(command, "full-root", at()).await,
        Err(GoalDbError::Busy)
    ));
    assert!(db
        .conversation_goal(&command.owner)
        .await
        .unwrap()
        .is_none());
    let actors: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_goal_actors")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(actors, 0);
}

async fn bound_report(db: &Database, command: &GoalMutation) -> GoalMutation {
    let snapshot = db
        .apply_conversation_goal(command, "full-root", at())
        .await
        .unwrap()
        .snapshot;
    live_turn(db).await;
    let execution = GoalExecution::parse(1, 1).unwrap();
    let binding = GoalExecutionBinding {
        owner: command.owner.clone(),
        execution,
        observation: GoalAdmissionObservation::Goal(snapshot.fence()),
    };
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    bind_goal_at_admission_tx(&mut tx, &binding).await.unwrap();
    tx.commit().await.unwrap();
    let source = GoalAgentSource {
        product_conversation_id: command.owner.clone(),
        transcript_id: t("full-root"),
        execution,
        message_id: t("report-message"),
        tool_use_id: t("report-call"),
    };
    let blocks = vec![phoenix_core::domain::llm_types::ContentBlock::ToolUse {
        id: source.tool_use_id.as_str().into(),
        name: "goal_report".into(),
        input: serde_json::json!({}),
    }];
    sqlx::query("INSERT INTO messages (conversation_id,message_id,sequence_id,message_type,content,created_at,origin_kind) VALUES ('full-root',?,2,'agent',?,'2026-10-19T00:00:01Z','unknown_historical')")
        .bind(source.message_id.as_str()).bind(serde_json::to_string(&blocks).unwrap())
        .execute(db.pool()).await.unwrap();
    GoalMutation {
        owner: command.owner.clone(),
        expected_version: snapshot.version,
        action_id: t("report-action"),
        attribution: GoalAttribution {
            requested_by: GoalActor::OwningAgent(source),
            applied_by: generated(),
        },
        change: GoalChange::Report {
            binding,
            disposition: GoalDisposition::Blocked(t("environment unavailable; restore access")),
        },
    }
}

#[tokio::test]
async fn exact_live_report_is_allowed_in_flight_but_owning_agent_create_is_busy() {
    let (db, command) = fixture().await;
    let report = bound_report(&db, &command).await;
    let current = db.conversation_goal(&command.owner).await.unwrap().unwrap();
    let set = replace(
        &current,
        report.attribution.requested_by.clone(),
        "agent-replacement",
    );
    assert!(matches!(
        db.apply_conversation_goal(&set, "full-root", at()).await,
        Err(GoalDbError::Busy)
    ));
    let blocked = db
        .apply_conversation_goal(&report, "full-root", at())
        .await
        .unwrap();
    assert!(matches!(blocked.snapshot.state, GoalState::Blocked(_)));
}

#[tokio::test]
async fn report_rejects_an_execution_that_no_longer_owns_the_turn() {
    let (db, command) = fixture().await;
    let report = bound_report(&db, &command).await;
    sqlx::query(
        "UPDATE durable_turns SET terminal_kind='Cancelled',owns_conversation=0 WHERE turn_id=1",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(matches!(
        db.apply_conversation_goal(&report, "full-root", at()).await,
        Err(GoalDbError::BindingConflict)
    ));
    assert_eq!(
        db.conversation_goal(&command.owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
}

#[tokio::test]
async fn settled_create_and_replace_advance_without_chat_or_execution() {
    let (db, command) = fixture().await;
    let first = db
        .apply_conversation_goal(&command, "full-root", at())
        .await
        .unwrap();
    let next = replace(
        &first.snapshot,
        command.attribution.requested_by.clone(),
        "replacement",
    );
    let second = db
        .apply_conversation_goal(&next, "full-root", at())
        .await
        .unwrap();
    assert_eq!(second.snapshot.version.get(), 2);
    assert_ne!(second.snapshot.goal_id, first.snapshot.goal_id);
    let messages: i64 = sqlx::query_scalar("SELECT count(*) FROM messages")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM durable_turns")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!((messages, turns), (1, 0));
}

#[tokio::test]
async fn active_turn_and_accepted_steering_each_prevent_creation_even_with_idle_projection() {
    let (db, command) = fixture().await;
    live_turn(&db).await;
    assert_busy(&db, &command).await;
    sqlx::query(
        "UPDATE durable_turns SET disposition='Steering',owns_conversation=0 WHERE turn_id=1",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert_busy(&db, &command).await;
}

#[tokio::test]
async fn exact_retry_is_historical_read_while_busy_but_new_payload_is_rejected() {
    let (db, command) = fixture().await;
    let accepted = db
        .apply_conversation_goal(&command, "full-root", at())
        .await
        .unwrap();
    live_turn(&db).await;
    let retry = db
        .apply_conversation_goal(&command, "full-root", GoalTimestampUs::parse(20).unwrap())
        .await
        .unwrap();
    assert_eq!(retry, accepted);
    let mut conflicting = command;
    conflicting.change = GoalChange::Create {
        goal_id: GoalId::parse("different").unwrap(),
        objective: t("different"),
    };
    assert!(matches!(
        db.apply_conversation_goal(&conflicting, "full-root", at())
            .await,
        Err(GoalDbError::RetryConflict)
    ));
}

#[tokio::test]
async fn waiting_questions_and_approval_remain_busy_without_a_live_turn() {
    use phoenix_core::domain::sm_state::ConvState;
    let (db, command) = fixture().await;
    let waiting_question: ConvState = serde_json::from_value(serde_json::json!({
        "type":"awaiting_user_response", "questions":[], "tool_use_id":"ask"
    }))
    .unwrap();
    let approval = ConvState::AwaitingTaskApproval {
        task_file: "tasks/plan.md".into(),
        title: "Plan".into(),
        priority: phoenix_core::task_source::Priority::P1,
        plan: "Review plan".into(),
    };
    for state in [waiting_question, approval] {
        sqlx::query("UPDATE conversations SET state=? WHERE id='full-root'")
            .bind(serde_json::to_string(&state).unwrap())
            .execute(db.pool())
            .await
            .unwrap();
        assert_busy(&db, &command).await;
    }
}

#[tokio::test]
async fn wrong_owner_and_stale_transcript_are_rechecked_before_write() {
    let (db, command) = fixture().await;
    assert!(db
        .apply_conversation_goal(&command, "not-the-latest", at())
        .await
        .is_err());
    let mut wrong_owner = command.clone();
    wrong_owner.owner = p("not-this-owner");
    assert!(matches!(
        db.apply_conversation_goal(&wrong_owner, "full-root", at())
            .await,
        Err(GoalDbError::TargetChanged)
    ));
    assert!(db
        .conversation_goal(&command.owner)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn pending_terminal_obligation_blocks_after_turn_owner_is_released() {
    let (db, command) = fixture().await;
    live_turn(&db).await;
    sqlx::query(
        "UPDATE durable_turns SET terminal_kind='Completed',owns_conversation=0 WHERE turn_id=1",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO direct_turn_terminal_obligations (turn_id,expected_generation,terminal_kind,target_state,target_state_updated_at_us) VALUES (1,0,'Completed',?,1)")
        .bind(serde_json::to_string(&phoenix_core::domain::sm_state::ConvState::Idle).unwrap())
        .execute(db.pool()).await.unwrap();
    assert_busy(&db, &command).await;
    sqlx::query("DELETE FROM direct_turn_terminal_obligations WHERE turn_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    db.apply_conversation_goal(&command, "full-root", at())
        .await
        .unwrap();
}

#[tokio::test]
async fn active_automatic_continuation_blocks_but_historical_terminal_admissions_do_not() {
    for phase in [
        "admitted",
        "successor_reserved",
        "ownership_transferred",
        "dispatch_accepted",
        "message_settled",
        "superseded",
        "failed",
    ] {
        let (db, command) = fixture().await;
        sqlx::query("INSERT INTO automatic_continuation_admissions (predecessor_conversation_id,product_conversation_id,summary_message_id,operation_id,first_message_id,phase,last_error,admitted_at_unix_micros,updated_at_unix_micros) VALUES ('full-root',?,'full-input','continue','first',?,?,1,1)")
            .bind(command.owner.as_str()).bind(phase)
            .bind(if phase == "failed" { Some("cannot continue") } else { None })
            .execute(db.pool()).await.unwrap();
        if matches!(phase, "message_settled" | "superseded" | "failed") {
            db.apply_conversation_goal(&command, "full-root", at())
                .await
                .unwrap();
        } else {
            assert_busy(&db, &command).await;
        }
    }
}

#[tokio::test]
async fn reserved_successor_blocks_a_goal_until_transfer_finishes() {
    let (db, command) = fixture().await;
    sqlx::query("INSERT INTO product_continuation_reservations (predecessor_conversation_id,successor_conversation_id,product_conversation_id) VALUES ('full-root','reserved-successor',?)")
        .bind(command.owner.as_str()).execute(db.pool()).await.unwrap();
    assert_busy(&db, &command).await;
    sqlx::query("DELETE FROM product_continuation_reservations WHERE predecessor_conversation_id='full-root'")
        .execute(db.pool()).await.unwrap();
    db.apply_conversation_goal(&command, "full-root", at())
        .await
        .unwrap();
}

#[tokio::test]
async fn queued_steering_blocks_even_without_a_durable_turn() {
    let (db, command) = fixture().await;
    sqlx::query("INSERT INTO steering_messages (message_id,conversation_id,ordinal,text) VALUES ('queued','full-root',0,'next input')")
        .execute(db.pool()).await.unwrap();
    assert_busy(&db, &command).await;
}

#[tokio::test]
async fn pending_opening_delivery_is_not_a_settled_creation() {
    for status in ["delivery_pending", "delivery_failed"] {
        let (db, command) = fixture().await;
        sqlx::query("INSERT INTO product_creation_jobs (request_id,product_conversation_id,cwd,objective,llm_language,status,accepted_at_unix_micros,updated_at_unix_micros,published_product_id,published_conversation_id) VALUES ('opening',?,'/tmp','opening','phoenix-native',?,1,1,?,'full-root')")
            .bind(command.owner.as_str()).bind(status).bind(command.owner.as_str())
            .execute(db.pool()).await.unwrap();
        assert!(db
            .apply_conversation_goal(&command, "full-root", at())
            .await
            .is_err());
        assert!(db
            .conversation_goal(&command.owner)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn transferred_latest_target_is_rechecked_and_unaccepted_opening_intent_blocks() {
    let (db, command) = fixture().await;
    sqlx::query("UPDATE conversations SET state=? WHERE id='full-root'")
        .bind(
            serde_json::to_string(
                &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                    summary: "continue source".into(),
                },
            )
            .unwrap(),
        )
        .execute(db.pool())
        .await
        .unwrap();
    let intent = crate::NewContinuationDispatchIntent::generated_predecessor_context(
        phoenix_workflow::ClientTurnKey::new("successor-opening").unwrap(),
        "continue source".into(),
    );
    let (outcome, _) = db
        .continue_conversation_with_intent("full-root", intent)
        .await
        .unwrap();
    let crate::ContinueOutcome::Created(successor) = outcome else {
        panic!("expected fresh continuation");
    };
    assert!(matches!(
        db.apply_conversation_goal(&command, "full-root", at())
            .await,
        Err(GoalDbError::TargetChanged)
    ));
    assert!(matches!(
        db.apply_conversation_goal(&command, &successor.id, at())
            .await,
        Err(GoalDbError::Busy)
    ));
    assert!(db
        .conversation_goal(&command.owner)
        .await
        .unwrap()
        .is_none());
}

async fn retain_report_source_for_round(db: &Database) -> String {
    let content =
        sqlx::query_scalar("SELECT content FROM messages WHERE message_id='report-message'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    sqlx::query("DELETE FROM messages WHERE message_id='report-message'")
        .execute(db.pool())
        .await
        .unwrap();
    content
}

async fn insert_complete_report_round(tx: &mut GoalTx<'_>, content: &str) {
    sqlx::query("INSERT INTO messages (conversation_id,message_id,sequence_id,message_type,content,created_at,origin_kind) VALUES ('full-root','report-message',2,'agent',?,'2026-10-19T00:00:01Z','system_generated')")
        .bind(content).execute(&mut **tx).await.unwrap();
    let result = phoenix_core::domain::db_schema::MessageContent::tool(
        "report-call",
        "checkpoint result",
        false,
    );
    sqlx::query("INSERT INTO messages (conversation_id,message_id,sequence_id,message_type,content,created_at,origin_kind) VALUES ('full-root','report-result',3,'tool',?,'2026-10-19T00:00:01Z','system_generated')")
        .bind(serde_json::to_string(&result).unwrap()).execute(&mut **tx).await.unwrap();
}

#[tokio::test]
async fn outer_round_rollback_removes_genuine_source_result_and_goal_change_together() {
    let (db, command) = fixture().await;
    let report = bound_report(&db, &command).await;
    let source = retain_report_source_for_round(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(matches!(
        apply_goal_tx(&mut tx, &report, Some("full-root"), at()).await,
        Err(GoalDbError::ActorMismatch)
    ));
    insert_complete_report_round(&mut tx, &source).await;
    let applied = apply_goal_tx(&mut tx, &report, Some("full-root"), at())
        .await
        .unwrap();
    assert!(matches!(applied.snapshot.state, GoalState::Blocked(_)));
    tx.rollback().await.unwrap();
    assert_eq!(
        db.conversation_goal(&command.owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        1
    );
    let source_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM messages WHERE message_id IN ('report-message','report-result')",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(source_rows, 0);
}

#[tokio::test]
async fn committed_outer_round_and_exact_report_retry_share_one_history_change() {
    let (db, command) = fixture().await;
    let report = bound_report(&db, &command).await;
    let source = retain_report_source_for_round(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    insert_complete_report_round(&mut tx, &source).await;
    let accepted = apply_goal_tx(&mut tx, &report, Some("full-root"), at())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let retried = db
        .apply_conversation_goal(&report, "full-root", GoalTimestampUs::parse(20).unwrap())
        .await
        .unwrap();
    assert_eq!(retried, accepted);
    assert_eq!(
        db.conversation_goal_history(&command.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        2
    );
    let source_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM messages WHERE message_id IN ('report-message','report-result')",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(source_rows, 2);
}
