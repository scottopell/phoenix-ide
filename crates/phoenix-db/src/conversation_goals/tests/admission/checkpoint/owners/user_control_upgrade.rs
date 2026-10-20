use super::*;
use crate::conversation_goals::control::GoalControlInstallation;
use phoenix_core::domain::conversation_goal::{
    GoalActor, GoalAttribution, GoalChange, GoalId, GoalMutation, GoalText, GoalTimestampUs,
    GoalUserCommandKind, GoalVersion,
};

const CANDIDATE: &str = include_str!("../../../../user_control_candidate.sql");

type ActorRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
);
type HistoryRow = (
    String,
    i64,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<i64>,
    i64,
    i64,
    i64,
);
type BindingRow = (i64, i64, String, Option<i64>, Option<String>);

#[derive(Debug, PartialEq, Eq)]
struct RetainedRows {
    actors: Vec<ActorRow>,
    history: Vec<HistoryRow>,
    bindings: Vec<BindingRow>,
    identities: Vec<(String, String, String)>,
}

async fn retained(db: &Database) -> RetainedRows {
    RetainedRows {
        actors: sqlx::query_as("SELECT actor_id,owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id,host_action_id FROM conversation_goal_actors ORDER BY actor_id").fetch_all(db.pool()).await.unwrap(),
        history: sqlx::query_as("SELECT owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us FROM conversation_goal_history ORDER BY owner_id,version").fetch_all(db.pool()).await.unwrap(),
        bindings: sqlx::query_as("SELECT turn_id,workflow_id,owner_id,observed_version,goal_id FROM conversation_goal_bindings ORDER BY turn_id").fetch_all(db.pool()).await.unwrap(),
        identities: sqlx::query_as("SELECT goal_id,objective,owner_id FROM conversation_goal_ids ORDER BY goal_id").fetch_all(db.pool()).await.unwrap(),
    }
}

async fn schema(db: &Database) -> Vec<(String, String)> {
    sqlx::query_as("SELECT name,sql FROM sqlite_master WHERE name LIKE 'conversation_goal_%' AND sql IS NOT NULL ORDER BY name")
        .fetch_all(db.pool()).await.unwrap()
}

async fn populated() -> (Database, GoalPendingReport) {
    let (db, assistant, report) =
        admitted_fixture_with_control(GoalControlInstallation::Installed).await;
    let results = [ToolRoundResult::PendingGoalReport {
        report: report.clone(),
        duration_ms: 1,
    }];
    let state = selected_state(Owner::Normal);
    let checkpoint = GoalRoundCheckpoint {
        assistant: &assistant,
        execution: report.execution(),
        results: &results,
        state: &state,
        state_updated_at: Utc::now(),
    };
    let (messages, replayed) = persist(Owner::Normal, &db, &checkpoint).await.unwrap();
    assert!(!replayed);
    let MessageContent::Tool(result) = &messages[0].content else {
        panic!("canonical tool result")
    };
    assert!(!result.is_error);
    let cycle: (i64, i64) = sqlx::query_as("SELECT h.report_turn_id,b.observed_version FROM conversation_goal_history h JOIN conversation_goal_bindings b ON b.turn_id=h.report_turn_id WHERE h.change_kind='report'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(cycle, (report.execution().turn_id(), 1));
    (db, report)
}

async fn upgrade(db: &Database) {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::raw_sql(CANDIDATE).execute(&mut *tx).await.unwrap();
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *tx)
        .await
        .unwrap()
        .is_empty());
    tx.commit().await.unwrap();
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(foreign_keys, 1);
    let staging: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_temp_master WHERE name LIKE 'goal_user_%'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(staging, 0);
}

#[tokio::test]
async fn conversation_goal_user_candidate_restores_populated_132_cycle_without_source_revalidation()
{
    for remove_source_messages in [false, true] {
        let (db, report) = populated().await;
        let before = retained(&db).await;
        let before_history = db
            .conversation_goal_history(&report.mutation().owner, GoalVersion::INITIAL)
            .await
            .unwrap();
        if remove_source_messages {
            sqlx::query("DELETE FROM messages WHERE conversation_id='full-root'")
                .execute(db.pool())
                .await
                .unwrap();
        }
        upgrade(&db).await;
        assert_eq!(retained(&db).await, before);
        assert_eq!(
            db.conversation_goal_history(&report.mutation().owner, GoalVersion::INITIAL)
                .await
                .unwrap(),
            before_history
        );
        let controls: i64 =
            sqlx::query_scalar("SELECT count(*) FROM conversation_goal_control_bindings")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(controls, 1);
        let requests: i64 =
            sqlx::query_scalar("SELECT count(*) FROM conversation_goal_user_requests")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(requests, 0, "upgrade must not fabricate user requests");
        db.delete_conversation("full-root").await.unwrap();
        for (table, query) in [
            (
                "conversation_goal_actors",
                "SELECT count(*) FROM conversation_goal_actors",
            ),
            (
                "conversation_goal_history",
                "SELECT count(*) FROM conversation_goal_history",
            ),
            (
                "conversation_goal_bindings",
                "SELECT count(*) FROM conversation_goal_bindings",
            ),
            (
                "conversation_goal_user_requests",
                "SELECT count(*) FROM conversation_goal_user_requests",
            ),
        ] {
            let count: i64 = sqlx::query_scalar(query)
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }
}

#[tokio::test]
async fn conversation_goal_user_candidate_mid_rebuild_failure_rolls_back_schema_cycle_and_staging()
{
    let (db, _) = populated().await;
    let before = retained(&db).await;
    let before_schema = schema(&db).await;
    let (prefix, _) = CANDIDATE
        .split_once("INSERT INTO conversation_goal_actors")
        .unwrap();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::raw_sql(prefix).execute(&mut *tx).await.unwrap();
    assert!(
        sqlx::query("SELECT * FROM deliberately_absent_upgrade_fault")
            .fetch_all(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    assert_eq!(retained(&db).await, before);
    assert_eq!(schema(&db).await, before_schema);
    let staging: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_temp_master WHERE name LIKE 'goal_user_%'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(staging, 0);
    upgrade(&db).await;
    assert_eq!(retained(&db).await, before);
}

#[tokio::test]
async fn conversation_goal_user_candidate_receipt_cannot_be_replaced_through_insert_conflict() {
    let (db, report) = populated().await;
    upgrade(&db).await;
    let owner = &report.mutation().owner;
    let goal = db.conversation_goal(owner).await.unwrap().unwrap();
    let request = "INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,outcome,result_version,rejected_state,rejected_reason,recorded_at_us) VALUES (?1,?2,?3,'state',1,?4,'rejected_version',2,'blocked','original',10)";
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query(request)
        .bind(owner.as_str())
        .bind(vec![1_u8; 16])
        .bind(vec![2_u8; 16])
        .bind(goal.goal_id.as_str())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(sqlx::query("INSERT OR REPLACE INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,outcome,result_version,rejected_state,rejected_reason,recorded_at_us) VALUES (?1,?2,?3,'state',1,?4,'rejected_version',2,'blocked','changed',99)")
        .bind(owner.as_str())
        .bind(vec![1_u8; 16])
        .bind(vec![2_u8; 16])
        .bind(goal.goal_id.as_str())
        .execute(db.pool())
        .await
        .is_err());
    let retained: (String, i64) = sqlx::query_as(
        "SELECT rejected_reason,recorded_at_us FROM conversation_goal_user_requests",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(retained, ("original".into(), 10));
}

async fn actor(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner: &str,
    principal: u8,
    request: u8,
    result_version: i64,
    command: &str,
    transcript: Option<&str>,
) -> std::result::Result<i64, sqlx::Error> {
    sqlx::query("INSERT INTO conversation_goal_actors (owner_id,kind,control_principal_id,control_request_id,control_command,transcript_id,control_result_version) VALUES (?1,'user_control',?2,?3,?4,?5,?6)")
        .bind(owner).bind(vec![principal;16]).bind(vec![request;16]).bind(command).bind(transcript).bind(result_version)
        .execute(&mut **tx).await.map(|result| result.last_insert_rowid())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_user_candidate_actor_requires_the_accepted_actual_goal_command() {
    let (db, report) = populated().await;
    upgrade(&db).await;
    let owner = report.mutation().owner;
    let other = db
        .create_conversation("other-root", "other", "/tmp", true, None, None)
        .await
        .unwrap()
        .product_conversation_id;
    let before = db.conversation_goal(&owner).await.unwrap().unwrap();
    control_command(
        &db,
        &owner,
        crate::conversation_goals::control::ControlCommand::Stop,
        1,
    )
    .await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,outcome,result_version,rejected_objective,rejected_state,recorded_at_us) VALUES (?1,?2,?3,'create',0,'rejected-id','rejected_version',2,'rejected objective','underway',10)")
        .bind(owner.as_str()).bind(vec![1_u8;16]).bind(vec![2_u8;16]).execute(&mut *tx).await.unwrap();
    assert!(actor(&mut tx, owner.as_str(), 1, 2, 2, "create", None)
        .await
        .is_err());
    sqlx::query("INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,outcome,result_version,recorded_at_us) VALUES (?1,?2,?3,'create',2,'control-created','accepted',3,10)")
        .bind(owner.as_str()).bind(vec![1_u8;16]).bind(vec![3_u8;16]).execute(&mut *tx).await.unwrap();
    for (target, principal, request, version, command, transcript) in [
        (other.as_str(), 1, 3, 3, "create", None),
        (owner.as_str(), 2, 3, 3, "create", None),
        (owner.as_str(), 1, 4, 3, "create", None),
        (owner.as_str(), 1, 3, 3, "state", None),
        (owner.as_str(), 1, 3, 2, "create", None),
        (owner.as_str(), 1, 3, 3, "create", Some("full-root")),
    ] {
        assert!(
            actor(&mut tx, target, principal, request, version, command, transcript)
                .await
                .is_err()
        );
    }
    let source = GoalActor::UserControl {
        principal_id: uuid::Uuid::from_bytes([1_u8; 16]),
        request_id: uuid::Uuid::from_bytes([3_u8; 16]),
        command: GoalUserCommandKind::Create,
    };
    let mutation = GoalMutation {
        owner: owner.clone(),
        expected_version: GoalVersion::parse(2).unwrap(),
        action_id: GoalText::parse("control-schema-create").unwrap(),
        attribution: GoalAttribution {
            requested_by: source.clone(),
            applied_by: source.clone(),
        },
        change: GoalChange::Create {
            goal_id: GoalId::parse("control-created").unwrap(),
            objective: GoalText::parse("canonical request objective").unwrap(),
        },
    };
    for requested in [true, false] {
        let mut wrong_command = mutation.clone();
        let wrong = GoalActor::UserControl {
            principal_id: uuid::Uuid::from_bytes([1_u8; 16]),
            request_id: uuid::Uuid::from_bytes([3_u8; 16]),
            command: GoalUserCommandKind::State,
        };
        if requested {
            wrong_command.attribution.requested_by = wrong;
        } else {
            wrong_command.attribution.applied_by = wrong;
        }
        assert_eq!(
            wrong_command.advance(Some(&before)),
            Err(phoenix_core::domain::conversation_goal::GoalError::Inapplicable)
        );
    }
    let entry = crate::conversation_goals::apply_goal_tx(
        &mut tx,
        &mutation,
        None,
        GoalTimestampUs::parse(10).unwrap(),
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
    assert_eq!(entry.mutation.attribution.requested_by, source);
    let actor: i64 = sqlx::query_scalar(
        "SELECT requested_actor_id FROM conversation_goal_history WHERE owner_id=? AND version=3",
    )
    .bind(owner.as_str())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *tx)
        .await
        .unwrap()
        .is_empty());
    tx.commit().await.unwrap();
    let current = db.conversation_goal(&owner).await.unwrap().unwrap();
    assert_eq!(current.version.get(), 3);
    assert_eq!(current.goal_id.as_str(), "control-created");
    assert_eq!(current.objective.as_str(), "canonical request objective");
    assert_ne!(before.goal_id, current.goal_id);
    let history = db
        .conversation_goal_history(&owner, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history.last(), Some(&entry));
    assert!(sqlx::query("INSERT OR REPLACE INTO conversation_goal_actors (actor_id,owner_id,kind,host_action_id) VALUES (?1,?2,'generated_host','relabel-user-control')")
        .bind(actor).bind(owner.as_str()).execute(db.pool()).await.is_err());
    let mut replacement = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(sqlx::query("INSERT OR REPLACE INTO conversation_goal_actors (owner_id,kind,control_principal_id,control_request_id,control_command,control_result_version) VALUES (?1,'user_control',?2,?3,'create',3)")
        .bind(owner.as_str()).bind(vec![1_u8;16]).bind(vec![3_u8;16]).execute(&mut *replacement).await.is_err());
    replacement.rollback().await.unwrap();
    assert_eq!(
        db.conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .last(),
        Some(&entry)
    );
    assert_eq!(
        serde_json::from_value::<GoalActor>(serde_json::to_value(&source).unwrap()).unwrap(),
        source
    );
    assert!(sqlx::query("INSERT INTO conversation_goal_history (owner_id,version,goal_id,state,reason,action_id,change_kind,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?1,4,'control-created','blocked','cannot reuse create','reuse','state',?2,?2,11)")
        .bind(owner.as_str()).bind(actor).execute(db.pool()).await.is_err());
    let receipt: (String,i64) = sqlx::query_as("SELECT command,result_version FROM conversation_goal_user_requests WHERE outcome='accepted'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(receipt, ("create".into(), 3));
    let stopped: (i64, i64) = sqlx::query_as(
        "SELECT control_version,stopped FROM conversation_goal_controls WHERE owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        stopped,
        (2, 1),
        "goal command must not release or advance Stop"
    );
    db.delete_conversation("full-root").await.unwrap();
    let remaining: (i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM conversation_goal_user_requests),(SELECT count(*) FROM conversation_goal_actors),(SELECT count(*) FROM conversation_goal_history),(SELECT count(*) FROM product_conversations WHERE id=?1)")
        .bind(other.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(remaining, (0, 0, 0, 1));
}
