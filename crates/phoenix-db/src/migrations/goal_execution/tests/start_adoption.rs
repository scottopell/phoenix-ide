mod review;

use super::*;
use crate::conversation_goals::start::GoalStartAcceptance;
use crate::workflow::{
    ClaimAuthoritativeTurnInput, GoalStartAdoptionInput, GoalStartAdoptionOutcome,
    LocalAuthorityResult,
};
use phoenix_core::domain::{conversation_goal::GoalTimestampUs, sm_state::ConvState};
use phoenix_workflow::{LeaseExpiry, ProcessIncarnation, Timestamp, TurnAuthorityId, WorkflowId};

async fn claimed() -> (Database, GoalStartAdoptionInput) {
    let (db, transcript, request) = start_acceptance::ready().await;
    sqlx::raw_sql(include_str!(
        "../../../conversation_goals/start_materialization_candidate.sql"
    ))
    .execute(db.pool())
    .await
    .unwrap();
    let GoalStartAcceptance::Accepted(receipt) = db
        .accept_owner_goal_start(&request, &transcript, GoalTimestampUs::parse(3).unwrap())
        .await
        .unwrap()
    else {
        panic!("fresh Start")
    };
    let repo = db.workflow_repository();
    let authority = repo
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id: TurnAuthorityId(u64::try_from(receipt.execution().turn_id()).unwrap()),
            workflow_id: WorkflowId(u64::try_from(receipt.execution().workflow_id()).unwrap()),
            process_incarnation: ProcessIncarnation(10),
            now: Timestamp(10),
            lease_until: LeaseExpiry(20),
        })
        .await
        .unwrap()
        .authority
        .unwrap();
    let start_id = repo
        .goal_start_id_for_receipt(&receipt)
        .await
        .unwrap()
        .unwrap();
    (
        db,
        GoalStartAdoptionInput {
            authority,
            start_id,
            receipt,
            state_updated_at: chrono::DateTime::from_timestamp(10, 0).unwrap(),
            now: Timestamp(10),
        },
    )
}

async fn materialization_counts(db: &Database) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM messages),(SELECT count(*) FROM conversation_goal_requests),(SELECT count(*) FROM conversation_goal_start_materializations),(SELECT count(*) FROM workflow_receipts)")
        .fetch_one(db.pool()).await.unwrap()
}

#[tokio::test]
async fn conversation_goal_start_materialization_charges_initial_state_once_without_message_or_origin(
) {
    let (db, input) = claimed().await;
    let before = materialization_counts(&db).await;
    let repo = db.workflow_repository();
    let result = repo.materialize_goal_start(&input).await;
    assert!(
        matches!(
            &result,
            LocalAuthorityResult::DurableFactEstablished(
                GoalStartAdoptionOutcome::Committed { .. }
            )
        ),
        "expected committed Start materialization, got {result:?}"
    );
    assert_eq!(
        materialization_counts(&db).await,
        (before.0, before.1 + 1, before.2 + 1, before.3 + 1)
    );
    let row: (i64,Option<String>,Option<String>,Option<Vec<u8>>) = sqlx::query_as("SELECT materialized,canonical_message_id,origin_kind,prepared_payload FROM durable_turns WHERE turn_id=?")
        .bind(input.receipt.execution().turn_id()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(row, (1, None, None, None));
    assert_eq!(
        db.get_conversation(input.receipt.historical_transcript().as_str())
            .await
            .unwrap()
            .state,
        ConvState::LlmRequesting { attempt: 1 }
    );
    assert!(matches!(
        repo.materialize_goal_start(&input).await,
        LocalAuthorityResult::DurableFactEstablished(GoalStartAdoptionOutcome::ExactReplay)
    ));
    assert_eq!(
        materialization_counts(&db).await,
        (before.0, before.1 + 1, before.2 + 1, before.3 + 1)
    );
    assert_eq!(
        repo.classify_goal_start_materialization(&input)
            .await
            .unwrap(),
        GoalStartAdoptionOutcome::ClassifiedCommitted {
            turn_id: TurnAuthorityId(u64::try_from(input.receipt.execution().turn_id()).unwrap()),
            generation: input.authority.generation.0,
        }
    );
}

#[tokio::test]
async fn conversation_goal_start_materialization_commit_loss_and_rollback_are_classified_from_full_bundle(
) {
    for after_commit in [false, true] {
        let (db, input) = claimed().await;
        let before = materialization_counts(&db).await;
        let result = db
            .workflow_repository()
            .materialize_goal_start_test_cut(&input, after_commit)
            .await;
        if after_commit {
            assert!(
                matches!(
                    &result,
                    LocalAuthorityResult::DurableFactEstablished(
                        GoalStartAdoptionOutcome::ClassifiedCommitted { .. }
                    )
                ),
                "expected classified committed Start after commit cut, got {result:?}"
            );
            assert_eq!(
                materialization_counts(&db).await,
                (before.0, before.1 + 1, before.2 + 1, before.3 + 1)
            );
        } else {
            assert!(
                matches!(
                    &result,
                    LocalAuthorityResult::DurableFactEstablished(
                        GoalStartAdoptionOutcome::NotCommitted
                    )
                ),
                "expected uncommitted Start before commit cut, got {result:?}"
            );
            assert_eq!(materialization_counts(&db).await, before);
            assert_eq!(
                db.get_conversation(input.receipt.historical_transcript().as_str())
                    .await
                    .unwrap()
                    .state,
                ConvState::Idle
            );
        }
    }
}

#[tokio::test]
async fn conversation_goal_start_witness_survives_live_cleanup_but_never_resurrects_execution() {
    let (db, input) = claimed().await;
    let repo = db.workflow_repository();
    let result = repo.materialize_goal_start(&input).await;
    assert!(
        matches!(
            &result,
            LocalAuthorityResult::DurableFactEstablished(
                GoalStartAdoptionOutcome::Committed { .. }
            )
        ),
        "expected committed Start materialization, got {result:?}"
    );
    let before = materialization_counts(&db).await;
    sqlx::query("DELETE FROM workflows WHERE workflow_id=?")
        .bind(input.receipt.execution().workflow_id())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        repo.classify_goal_start_materialization(&input)
            .await
            .unwrap(),
        GoalStartAdoptionOutcome::CommittedElsewhere
    );
    let retained = db
        .replay_owner_goal_start(input.receipt.input())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.live_turn_id(), None);
    assert_eq!(retained.execution(), input.receipt.execution());
    let after = materialization_counts(&db).await;
    assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
    assert!(matches!(
        repo.materialize_goal_start(&input).await,
        LocalAuthorityResult::DurableFactEstablished(GoalStartAdoptionOutcome::CommittedElsewhere)
    ));
    assert_eq!(materialization_counts(&db).await, after);
}

#[tokio::test]
async fn conversation_goal_start_materialization_witness_rejects_raw_forgery_replace_update_and_delete(
) {
    let (db, input) = claimed().await;
    let forged = "INSERT INTO conversation_goal_start_materializations(start_id,request_id,effect_id,attempt_id,process_incarnation,state_updated_at) VALUES (?,?,?,?,?,?)";
    assert!(sqlx::query(forged)
        .bind(i64::try_from(input.start_id.get()).unwrap())
        .bind(input.intent().unwrap().id.as_str())
        .bind(i64::try_from(input.authority.effect_id.0).unwrap())
        .bind(i64::try_from(input.authority.attempt_id.0).unwrap())
        .bind(10_i64)
        .bind(input.state_updated_at.to_rfc3339())
        .execute(db.pool())
        .await
        .is_err());
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
    for statement in [
        "UPDATE conversation_goal_start_materializations SET process_incarnation=999",
        "DELETE FROM conversation_goal_start_materializations",
        "INSERT OR REPLACE INTO conversation_goal_start_materializations SELECT start_id,request_id,effect_id,attempt_id,999,state_updated_at FROM conversation_goal_start_materializations",
    ] {
        assert!(sqlx::query(statement).execute(db.pool()).await.is_err(),"{statement}");
    }
    sqlx::query("UPDATE workflow_receipts SET receipt_payload=? WHERE workflow_id=?")
        .bind(b"{}".as_slice())
        .bind(input.receipt.execution().workflow_id())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db
        .workflow_repository()
        .classify_goal_start_materialization(&input)
        .await
        .is_err());
}

#[tokio::test]
async fn conversation_goal_start_request_context_reuses_admitted_initial_identity_without_recharging(
) {
    let (db, input) = claimed().await;
    let repo = db.workflow_repository();
    let result = repo.materialize_goal_start(&input).await;
    assert!(
        matches!(
            &result,
            LocalAuthorityResult::DurableFactEstablished(
                GoalStartAdoptionOutcome::Committed { .. }
            )
        ),
        "expected committed Start materialization, got {result:?}"
    );
    let before = materialization_counts(&db).await;
    for _ in 0..2 {
        let context = repo
            .goal_start_request_context(
                input.receipt.historical_transcript().as_str(),
                TurnAuthorityId(u64::try_from(input.receipt.execution().turn_id()).unwrap()),
                input.authority.generation.0,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(context.request_id, input.intent().unwrap().id);
        assert_eq!(&context.observation, input.receipt.observation());
        assert!(context
            .system_context()
            .contains(context.observation.objective.as_str()));
    }
    assert_eq!(materialization_counts(&db).await, before);
    sqlx::query("UPDATE conversations SET state_updated_at=? WHERE id=?")
        .bind(
            chrono::DateTime::from_timestamp(11, 0)
                .unwrap()
                .to_rfc3339(),
        )
        .bind(input.receipt.historical_transcript().as_str())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(repo
        .goal_start_request_context(
            input.receipt.historical_transcript().as_str(),
            TurnAuthorityId(u64::try_from(input.receipt.execution().turn_id()).unwrap()),
            input.authority.generation.0
        )
        .await
        .is_err());
    assert_eq!(materialization_counts(&db).await, before);
}

#[tokio::test]
async fn conversation_goal_start_missing_witness_after_live_advance_is_not_known_uncommitted() {
    let (db, input) = claimed().await;
    sqlx::query("UPDATE durable_turns SET materialized=1 WHERE turn_id=?")
        .bind(input.receipt.execution().turn_id())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db
        .workflow_repository()
        .classify_goal_start_materialization(&input)
        .await
        .is_err());
}
