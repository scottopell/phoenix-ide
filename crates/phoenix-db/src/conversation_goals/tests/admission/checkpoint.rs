use crate::conversation_goals::control::GoalControlInstallation;
use phoenix_core::domain::conversation_goal::{
    GoalChange, GoalDisposition, GoalExecution, GoalMutation, GoalPendingReport, GoalReportCall,
    GoalReportOutcome, GoalVersion,
};
use phoenix_core::domain::db_schema::{Message, MessageContent, ToolResult};
use phoenix_core::domain::llm_types::ContentBlock;

use crate::conversation_goals::{checkpoint::finalize_report_tx, GoalDbError, GoalTx};
use crate::Database;

use super::{at, bound_report, replace, t};

#[path = "checkpoint/round.rs"]
mod round;

#[path = "checkpoint/owners.rs"]
mod owners;

async fn pending_fixture(
    outcome: GoalReportOutcome,
) -> (Database, GoalMutation, GoalPendingReport) {
    let (db, command) = super::fixture().await;
    let prepared = bound_report(&db, &command).await;
    let GoalChange::Report { binding, .. } = prepared.change else {
        panic!("report fixture")
    };
    let pending = GoalPendingReport::new(
        binding.try_into().unwrap(),
        GoalReportCall {
            transcript_id: t("full-root"),
            message_id: t("report-message"),
            tool_use_id: t("report-call"),
        },
        outcome,
    );
    let blocks = vec![ContentBlock::ToolUse {
        id: "report-call".into(),
        name: "goal_report".into(),
        input: serde_json::to_value(pending.outcome()).unwrap(),
    }];
    sqlx::query("UPDATE messages SET content=? WHERE message_id='report-message'")
        .bind(serde_json::to_string(&blocks).unwrap())
        .execute(db.pool())
        .await
        .unwrap();
    (db, command, pending)
}

async fn round_assistant(db: &Database) -> Message {
    db.get_messages("full-root")
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.message_id == "report-message")
        .unwrap()
}

async fn store_result(tx: &mut GoalTx<'_>, id: &str, sequence: i64, result: &ToolResult) {
    assert!(
        result.images().is_empty(),
        "this report fixture stores text-only results"
    );
    let content = MessageContent::tool(&result.tool_use_id, result.output(), result.is_error());
    let display = result
        .duration_ms
        .map(|duration| serde_json::json!({"duration_ms":duration}).to_string());
    sqlx::query("INSERT INTO messages (conversation_id,message_id,sequence_id,message_type,content,display_data,created_at,origin_kind) VALUES ('full-root',?,?,'tool',?,?,'2026-10-19T00:00:02Z','system_generated')")
        .bind(id).bind(sequence).bind(serde_json::to_string(&content).unwrap()).bind(display)
        .execute(&mut **tx).await.unwrap();
}

async fn settle_fixture_turn(db: &Database) {
    sqlx::query(
        "UPDATE durable_turns SET terminal_kind='Completed',owns_conversation=0 WHERE turn_id=1",
    )
    .execute(db.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn report_and_paired_result_remain_uncommitted_until_the_outer_owner_commits() {
    let (db, command, pending) = pending_fixture(GoalReportOutcome::Complete(t("done"))).await;
    let assistant = round_assistant(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let result = finalize_report_tx(
        &mut tx,
        &assistant,
        GoalExecution::parse(1, 1).unwrap(),
        &pending,
        7,
        at(),
        GoalControlInstallation::Disabled,
    )
    .await
    .unwrap();
    assert!(!result.is_error());
    store_result(
        &mut tx,
        "tool-result:14:report-message:report-call",
        3,
        &result,
    )
    .await;
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
    let result_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM messages WHERE message_id='tool-result:14:report-message:report-call'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(result_rows, 0);
    assert_eq!(
        db.conversation_goal_history(&command.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn committed_success_response_loss_reuses_the_original_result_after_replacement() {
    let (db, command, pending) = pending_fixture(GoalReportOutcome::Complete(t("done"))).await;
    let assistant = round_assistant(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let original = finalize_report_tx(
        &mut tx,
        &assistant,
        GoalExecution::parse(1, 1).unwrap(),
        &pending,
        7,
        at(),
        GoalControlInstallation::Disabled,
    )
    .await
    .unwrap();
    store_result(
        &mut tx,
        "tool-result:14:report-message:report-call",
        3,
        &original,
    )
    .await;
    tx.commit().await.unwrap();
    settle_fixture_turn(&db).await;
    let completed = db.conversation_goal(&command.owner).await.unwrap().unwrap();
    let replacement = replace(
        &completed,
        command.attribution.requested_by.clone(),
        "after-completion",
    );
    let new_goal = db
        .apply_conversation_goal(&replacement, "full-root", at())
        .await
        .unwrap()
        .snapshot;
    let assistant = round_assistant(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let recovered = finalize_report_tx(
        &mut tx,
        &assistant,
        GoalExecution::parse(1, 1).unwrap(),
        &pending,
        99,
        at(),
        GoalControlInstallation::Disabled,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(recovered.output(), original.output());
    assert_eq!(recovered.is_error(), original.is_error());
    assert_eq!(recovered.duration_ms, Some(7));
    assert_eq!(
        db.conversation_goal(&command.owner).await.unwrap(),
        Some(new_goal)
    );
    assert_eq!(
        db.conversation_goal_history(&command.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        3
    );
}

async fn install_semantic_refusal_calls(
    db: &Database,
    first: &GoalPendingReport,
    pending: &GoalPendingReport,
) {
    let blocks = vec![
        ContentBlock::ToolUse {
            id: "prior-call".into(),
            name: "goal_report".into(),
            input: serde_json::to_value(first.outcome()).unwrap(),
        },
        ContentBlock::ToolUse {
            id: "unrelated-call".into(),
            name: "think".into(),
            input: serde_json::json!({"thoughts":"unrelated"}),
        },
        ContentBlock::ToolUse {
            id: "report-call".into(),
            name: "goal_report".into(),
            input: serde_json::to_value(pending.outcome()).unwrap(),
        },
    ];
    sqlx::query("UPDATE messages SET content=? WHERE message_id='report-message'")
        .bind(serde_json::to_string(&blocks).unwrap())
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn semantic_refusal_preserves_other_results_and_its_committed_rejection_on_replay() {
    let (db, command, pending) = pending_fixture(GoalReportOutcome::Complete(t("done"))).await;
    let GoalChange::Report { binding, .. } = pending.mutation().change else {
        panic!("report")
    };
    let first = GoalPendingReport::new(
        binding.try_into().unwrap(),
        GoalReportCall {
            transcript_id: t("full-root"),
            message_id: t("report-message"),
            tool_use_id: t("prior-call"),
        },
        GoalReportOutcome::Blocked(t("access unavailable")),
    );
    install_semantic_refusal_calls(&db, &first, &pending).await;
    let assistant = round_assistant(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let accepted = finalize_report_tx(
        &mut tx,
        &assistant,
        GoalExecution::parse(1, 1).unwrap(),
        &first,
        1,
        at(),
        GoalControlInstallation::Disabled,
    )
    .await
    .unwrap();
    store_result(
        &mut tx,
        "tool-result:14:report-message:prior-call",
        3,
        &accepted,
    )
    .await;
    let unrelated = ToolResult::success("unrelated-call".into(), "unrelated output".into());
    store_result(&mut tx, "unrelated-result", 4, &unrelated).await;
    let rejected = finalize_report_tx(
        &mut tx,
        &assistant,
        GoalExecution::parse(1, 1).unwrap(),
        &pending,
        2,
        at(),
        GoalControlInstallation::Disabled,
    )
    .await
    .unwrap();
    assert!(rejected.is_error());
    store_result(
        &mut tx,
        "tool-result:14:report-message:report-call",
        5,
        &rejected,
    )
    .await;
    tx.commit().await.unwrap();
    settle_fixture_turn(&db).await;
    let blocked = db.conversation_goal(&command.owner).await.unwrap().unwrap();
    let resume = GoalMutation {
        owner: command.owner.clone(),
        expected_version: blocked.version,
        action_id: t("managed-resume"),
        attribution: command.attribution,
        change: GoalChange::SetDisposition {
            goal_id: blocked.goal_id,
            disposition: GoalDisposition::Underway,
        },
    };
    db.apply_conversation_goal(&resume, "full-root", at())
        .await
        .unwrap();
    let assistant = round_assistant(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let recovered = finalize_report_tx(
        &mut tx,
        &assistant,
        GoalExecution::parse(1, 1).unwrap(),
        &pending,
        5,
        at(),
        GoalControlInstallation::Disabled,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(recovered.is_error());
    assert_eq!(recovered.output(), rejected.output());
    let unrelated_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM messages WHERE message_id='unrelated-result'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(unrelated_rows, 1);
    assert_eq!(
        db.conversation_goal_history(&command.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn existing_result_is_not_replay_authority_for_changed_genuine_tool_arguments() {
    let (db, command, original) = pending_fixture(GoalReportOutcome::Complete(t("original"))).await;
    let GoalChange::Report { binding, .. } = original.mutation().change else {
        panic!("report")
    };
    let altered = GoalPendingReport::new(
        binding.try_into().unwrap(),
        original.call().clone(),
        GoalReportOutcome::Blocked(t("changed body")),
    );
    let assistant = round_assistant(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    store_result(
        &mut tx,
        "tool-result:14:report-message:report-call",
        3,
        &ToolResult::error("report-call".into(), "original rejection".into()),
    )
    .await;
    assert!(matches!(
        finalize_report_tx(
            &mut tx,
            &assistant,
            GoalExecution::parse(1, 1).unwrap(),
            &altered,
            1,
            at(),
            GoalControlInstallation::Disabled,
        )
        .await,
        Err(GoalDbError::Integrity("pending report changed arguments"))
    ));
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
}

#[tokio::test]
async fn report_cannot_borrow_another_round_or_execution_even_in_the_same_transcript() {
    let (db, command, pending) = pending_fixture(GoalReportOutcome::Complete(t("done"))).await;
    let assistant = round_assistant(&db).await;
    let mut other_round = assistant.clone();
    other_round.message_id = "another-genuine-round".into();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    for (source, execution) in [
        (&other_round, GoalExecution::parse(1, 1).unwrap()),
        (&assistant, GoalExecution::parse(1, 2).unwrap()),
    ] {
        assert!(matches!(
            finalize_report_tx(
                &mut tx,
                source,
                execution,
                &pending,
                1,
                at(),
                GoalControlInstallation::Disabled
            )
            .await,
            Err(GoalDbError::Integrity("pending report owning round"))
        ));
    }
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
}

#[tokio::test]
async fn retained_success_must_match_exact_accepted_history_not_just_the_success_flag() {
    let (db, _, pending) = pending_fixture(GoalReportOutcome::Complete(t("done"))).await;
    let assistant = round_assistant(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let mut result = finalize_report_tx(
        &mut tx,
        &assistant,
        GoalExecution::parse(1, 1).unwrap(),
        &pending,
        1,
        at(),
        GoalControlInstallation::Disabled,
    )
    .await
    .unwrap();
    result.outcome = ToolResult::success("report-call".into(), "different success".into()).outcome;
    store_result(
        &mut tx,
        "tool-result:14:report-message:report-call",
        3,
        &result,
    )
    .await;
    tx.commit().await.unwrap();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(matches!(
        finalize_report_tx(
            &mut tx,
            &assistant,
            GoalExecution::parse(1, 1).unwrap(),
            &pending,
            9,
            at(),
            GoalControlInstallation::Disabled,
        )
        .await,
        Err(GoalDbError::Integrity(
            "pending report retained success differs"
        ))
    ));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn unknown_constraint_failure_aborts_the_outer_round_instead_of_becoming_tool_error() {
    let (db, command, pending) = pending_fixture(GoalReportOutcome::Complete(t("done"))).await;
    sqlx::query("CREATE TRIGGER fail_goal_report_history BEFORE INSERT ON conversation_goal_history WHEN NEW.change_kind='report' BEGIN SELECT RAISE(ABORT,'unexpected checkpoint constraint'); END")
        .execute(db.pool()).await.unwrap();
    let assistant = round_assistant(&db).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    store_result(
        &mut tx,
        "unrelated-result",
        3,
        &ToolResult::success("other".into(), "other output".into()),
    )
    .await;
    assert!(matches!(
        finalize_report_tx(
            &mut tx,
            &assistant,
            GoalExecution::parse(1, 1).unwrap(),
            &pending,
            1,
            at(),
            GoalControlInstallation::Disabled,
        )
        .await,
        Err(GoalDbError::Sqlx(_))
    ));
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
    let unrelated_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM messages WHERE message_id='unrelated-result'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(unrelated_rows, 0);
}
