use super::*;
use crate::workflow::{GoalStartRecoveryInput, GoalStartRecoveryOutcome};
use phoenix_core::domain::conversation_goal::GoalRequestOperation;

fn recovery(input: &GoalStartAdoptionInput) -> GoalStartRecoveryInput {
    GoalStartRecoveryInput::new(
        input.receipt.historical_transcript().as_str().to_owned(),
        (
            TurnAuthorityId(u64::try_from(input.receipt.execution().turn_id()).unwrap()),
            input.authority.generation.0,
        ),
        (
            ConvState::LlmRequesting { attempt: 1 },
            input.state_updated_at,
        ),
        Timestamp(30),
    )
}

async fn ownership(db: &Database, input: &GoalStartAdoptionInput) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT t.generation,w.generation,w.version,
            (SELECT count(*) FROM workflow_transitions r WHERE r.workflow_id=w.workflow_id)
         FROM durable_turns t JOIN workflows w ON w.workflow_id=t.workflow_id WHERE t.turn_id=?",
    )
    .bind(input.receipt.execution().turn_id())
    .fetch_one(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn conversation_goal_reconstruction_race_has_one_owner_not_winner_adoption() {
    let (db, adopted) = materialized().await;
    let before = materialization_counts(&db).await;
    let left = recovery(&adopted);
    let right = recovery(&adopted);
    let repo = db.workflow_repository();
    let (left_result, right_result) = tokio::join!(
        repo.recover_goal_start_runtime(&left),
        repo.recover_goal_start_runtime(&right),
    );
    let outcomes = [left_result.unwrap(), right_result.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, GoalStartRecoveryOutcome::Recovered(_)))
            .count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == GoalStartRecoveryOutcome::Refused)
            .count(),
        1
    );
    let owned = outcomes
        .iter()
        .find_map(|outcome| match outcome {
            GoalStartRecoveryOutcome::Recovered(owned) => Some(owned),
            GoalStartRecoveryOutcome::Refused => None,
        })
        .unwrap();
    assert_eq!(owned.generation(), adopted.authority.generation.0 + 1);
    assert_eq!(ownership(&db, &adopted).await, (1, 1, 3, 3));
    assert_eq!(
        repo.recover_goal_start_runtime(&left).await.unwrap(),
        GoalStartRecoveryOutcome::Refused
    );
    assert_eq!(
        repo.recover_goal_start_runtime(&right).await.unwrap(),
        GoalStartRecoveryOutcome::Refused
    );
    assert!(context(&db, &adopted).await.is_err());
    let restored = repo
        .admit_goal_start_request_context(
            adopted.receipt.historical_transcript().as_str(),
            Some((owned.turn_id(), owned.generation())),
            adopted.state_updated_at,
            GoalRequestOperation::Normal { attempt: 1 },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.request_id(), &adopted.intent().unwrap().id);
    assert_eq!(restored.admitted().intent().generation(), 0);
    assert_eq!(restored.admitted().claim_generation(), 1);
    assert_eq!(materialization_counts(&db).await, before);
    let lease_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow_reclaimable_leases WHERE workflow_id=?")
            .bind(adopted.receipt.execution().workflow_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(lease_count, 0);
}

#[tokio::test]
async fn conversation_goal_reconstruction_commit_cuts_prove_whole_bundle_or_no_advance() {
    for after_commit in [false, true] {
        let (db, adopted) = materialized().await;
        let before = ownership(&db, &adopted).await;
        let counts = materialization_counts(&db).await;
        let repo = db.workflow_repository();
        let input = recovery(&adopted);
        let result = repo
            .recover_goal_start_runtime_test_cut(&input, after_commit)
            .await;
        if after_commit {
            let GoalStartRecoveryOutcome::Recovered(owned) = result.unwrap() else {
                panic!("exact committed reconstruction must classify");
            };
            assert_eq!(owned.generation(), 1);
            assert_eq!(
                ownership(&db, &adopted).await,
                (1, 1, before.2 + 1, before.3 + 1)
            );
            assert_eq!(
                repo.recover_goal_start_runtime(&input).await.unwrap(),
                GoalStartRecoveryOutcome::Refused
            );
        } else {
            assert!(result.is_err());
            assert_eq!(ownership(&db, &adopted).await, before);
            assert!(matches!(
                repo.recover_goal_start_runtime(&input).await.unwrap(),
                GoalStartRecoveryOutcome::Recovered(_)
            ));
        }
        assert_eq!(materialization_counts(&db).await, counts);
    }
}

#[tokio::test]
async fn conversation_goal_reconstruction_turn_write_failure_rolls_back_workflow_transition() {
    let (db, adopted) = materialized().await;
    let before = ownership(&db, &adopted).await;
    let counts = materialization_counts(&db).await;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_reconstruction BEFORE UPDATE OF generation ON durable_turns
        WHEN NEW.turn_id=OLD.turn_id AND NEW.generation>OLD.generation
        BEGIN SELECT RAISE(ABORT,'injected reconstruction turn failure'); END;",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(db
        .workflow_repository()
        .recover_goal_start_runtime(&recovery(&adopted))
        .await
        .is_err());
    assert_eq!(ownership(&db, &adopted).await, before);
    assert_eq!(materialization_counts(&db).await, counts);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_reconstruction_rejects_changed_slot_idle_checkpoint_and_closed_owner() {
    for fence in [
        "slot",
        "idle",
        "checkpoint",
        "history",
        "close",
        "stale_latest",
        "queued",
        "terminal",
    ] {
        let (db, adopted) = materialized().await;
        let mut request = recovery(&adopted);
        let transcript = adopted.receipt.historical_transcript().as_str();
        match fence {
            "slot" => {
                db.update_conversation_state_at(
                    transcript,
                    &ConvState::LlmRequesting { attempt: 1 },
                    adopted.state_updated_at + chrono::Duration::microseconds(1),
                )
                .await
                .unwrap();
            }
            "idle" => {
                db.update_conversation_state(transcript, &ConvState::Idle)
                    .await
                    .unwrap();
                let current = db.get_conversation(transcript).await.unwrap();
                request = GoalStartRecoveryInput::new(
                    transcript.to_owned(),
                    (
                        TurnAuthorityId(
                            u64::try_from(adopted.receipt.execution().turn_id()).unwrap(),
                        ),
                        adopted.authority.generation.0,
                    ),
                    (current.state, current.state_updated_at),
                    Timestamp(30),
                );
            }
            "checkpoint" => {
                let mut tx = db.pool().begin().await.unwrap();
                crate::conversation_goals::execution::checkpoint_request_tx(
                    &mut tx,
                    db.goal_execution_installation(),
                    &adopted.intent().unwrap(),
                    i64::try_from(adopted.authority.generation.0).unwrap(),
                    &phoenix_core::domain::conversation_goal::GoalText::parse("completed-response")
                        .unwrap(),
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
            }
            "history" => {
                sqlx::query(
                    "UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id=?",
                )
                .bind(adopted.receipt.input().goal().owner.as_str())
                .execute(db.pool())
                .await
                .unwrap();
            }
            "close" => {
                db.begin_close_foundation(
                    &adopted.receipt.input().goal().owner,
                    &phoenix_core::domain::close::TranscriptConversationId::parse(transcript)
                        .unwrap(),
                    "goal-reconstruction-close",
                )
                .await
                .unwrap();
                assert!(!db
                    .product_conversation_admission(transcript)
                    .await
                    .unwrap()
                    .is_accepted());
            }
            "stale_latest" => {
                db.update_conversation_state(
                    transcript,
                    &ConvState::ContextExhausted {
                        summary: "continue the retained goal".into(),
                    },
                )
                .await
                .unwrap();
                let crate::ContinueOutcome::Created(successor) =
                    db.continue_conversation(transcript).await.unwrap()
                else {
                    panic!("fixture must create a real successor transcript");
                };
                assert_eq!(
                    successor.product_conversation_id,
                    adopted.receipt.input().goal().owner
                );
                let aggregate = db
                    .get_ordinary_product_conversation(&adopted.receipt.input().goal().owner)
                    .await
                    .unwrap();
                assert_eq!(aggregate.latest_transcript_row_id, successor.id);
                assert_ne!(aggregate.latest_transcript_row_id, transcript);
                let lifecycle: String = sqlx::query_scalar(
                    "SELECT ordinary_lifecycle FROM product_conversations WHERE id=?",
                )
                .bind(adopted.receipt.input().goal().owner.as_str())
                .fetch_one(db.pool())
                .await
                .unwrap();
                assert_eq!(lifecycle, "open");
                let mut tx = db.pool().begin().await.unwrap();
                assert!(matches!(
                    crate::conversation_goals::execution::require_live_request_tx(
                        &mut tx,
                        &adopted.intent().unwrap(),
                        transcript,
                    )
                    .await,
                    Err(
                        crate::conversation_goals::execution::GoalRequestError::Goal(
                            crate::conversation_goals::GoalDbError::TargetChanged
                        )
                    )
                ));
                tx.rollback().await.unwrap();
            }
            "terminal" => {
                db.workflow_repository()
                    .persist_terminal_obligation(
                        &crate::workflow::DirectTurnTerminalObligationInput {
                            turn_id: TurnAuthorityId(
                                u64::try_from(adopted.receipt.execution().turn_id()).unwrap(),
                            ),
                            expected_generation: adopted.authority.generation.0,
                            terminal: phoenix_workflow::TurnTerminal::Cancelled,
                            projection: crate::workflow::PersistedConversationProjection {
                                state: ConvState::Idle,
                                state_updated_at: adopted.state_updated_at,
                            },
                            response_message_id: None,
                        },
                    )
                    .await
                    .unwrap();
            }
            "queued" => {
                let accepted = phoenix_core::domain::sm_event::SteerEntry {
                    message_id: "reconstruction-accepted-input".into(),
                    text: "accepted input has priority".into(),
                    origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
                    llm_text: None,
                    images: Vec::new(),
                    files: Vec::new(),
                    user_agent: None,
                    skill_invocation: None,
                };
                db.append_steering_entry(transcript, &accepted, "reconstruction-input-fingerprint")
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = ownership(&db, &adopted).await;
        let counts = materialization_counts(&db).await;
        assert_eq!(
            db.workflow_repository()
                .recover_goal_start_runtime(&request)
                .await
                .unwrap(),
            GoalStartRecoveryOutcome::Refused,
            "{fence}"
        );
        assert_eq!(ownership(&db, &adopted).await, before, "{fence}");
        assert_eq!(materialization_counts(&db).await, counts, "{fence}");
    }
}

#[tokio::test]
async fn conversation_goal_reconstruction_stop_release_does_not_refresh_control_binding() {
    use crate::conversation_goals::control::{ControlCommand, ControlRequest};
    let (db, adopted) = materialized().await;
    let before = ownership(&db, &adopted).await;
    let counts = materialization_counts(&db).await;
    for (command, version) in [(ControlCommand::Stop, 1), (ControlCommand::Release, 2)] {
        db.apply_goal_control(
            &ControlRequest {
                owner: adopted.receipt.input().goal().owner.clone(),
                principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
                request_id: uuid::Uuid::new_v4(),
                command,
                expected_control_version: version,
            },
            GoalTimestampUs::parse(50 + i64::try_from(version).unwrap()).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            db.workflow_repository()
                .recover_goal_start_runtime(&recovery(&adopted))
                .await
                .unwrap(),
            GoalStartRecoveryOutcome::Refused
        );
        assert_eq!(ownership(&db, &adopted).await, before);
        assert_eq!(materialization_counts(&db).await, counts);
    }
}

#[tokio::test]
async fn conversation_goal_startup_reset_retains_admitted_slot_for_reconstruction() {
    let (db, adopted) = materialized().await;
    let before = db
        .get_conversation(adopted.receipt.historical_transcript().as_str())
        .await
        .unwrap();
    db.reset_all_to_idle().await.unwrap();
    let after = db
        .get_conversation(adopted.receipt.historical_transcript().as_str())
        .await
        .unwrap();
    assert_eq!(after.state, before.state);
    assert_eq!(after.state_updated_at, before.state_updated_at);
    assert!(matches!(
        db.workflow_repository()
            .recover_goal_start_runtime(&recovery(&adopted))
            .await
            .unwrap(),
        GoalStartRecoveryOutcome::Recovered(_)
    ));
}

#[tokio::test]
async fn conversation_goal_reconstruction_disabled_does_not_read_candidate_request_schema() {
    let (mut db, adopted) = materialized().await;
    db.goal_execution_installation =
        crate::conversation_goals::execution::GoalExecutionInstallation::Disabled;
    sqlx::query("DROP TABLE conversation_goal_start_materializations")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.workflow_repository()
            .recover_goal_start_runtime(&recovery(&adopted))
            .await
            .unwrap(),
        GoalStartRecoveryOutcome::Refused
    );
}
