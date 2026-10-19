use super::*;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

fn p(value: &str) -> ProductConversationId {
    ProductConversationId::parse(value).unwrap()
}

fn t(value: &str) -> GoalText {
    GoalText::parse(value).unwrap()
}

fn at() -> GoalTimestampUs {
    GoalTimestampUs::parse(10).unwrap()
}

async fn fixture() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("goals.db");
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    // Minimal substrate isolates the goal DDL; a separate test opens the full migration set.
    sqlx::raw_sql("
        CREATE TABLE product_conversations (id TEXT PRIMARY KEY,kind TEXT NOT NULL);
        CREATE TABLE conversations (id TEXT PRIMARY KEY,product_conversation_id TEXT NOT NULL REFERENCES product_conversations(id));
        CREATE TABLE messages (conversation_id TEXT NOT NULL REFERENCES conversations(id),message_id TEXT NOT NULL,origin_kind TEXT NOT NULL,message_type TEXT NOT NULL,content TEXT NOT NULL,PRIMARY KEY(conversation_id,message_id));
        CREATE TABLE workflows (workflow_id INTEGER PRIMARY KEY);
        CREATE TABLE durable_turns (turn_id INTEGER PRIMARY KEY,workflow_id INTEGER NOT NULL REFERENCES workflows(workflow_id),conversation_id TEXT NOT NULL REFERENCES conversations(id),disposition TEXT NOT NULL,terminal_kind TEXT,canonical_message_id TEXT,owns_conversation INTEGER NOT NULL);
        INSERT INTO product_conversations VALUES ('owner','ordinary'),('other','ordinary'),('global','coordinator');
        INSERT INTO conversations VALUES ('root','owner'),('successor','owner'),('other-root','other'),('global-root','global');
        INSERT INTO messages VALUES ('root','user-input','user_api','user','{}'),('root','agent-call','unknown_historical','agent','[]'),('global-root','global-call','unknown_historical','agent','[]');
        INSERT INTO workflows VALUES (1),(2),(3),(4);
        INSERT INTO durable_turns VALUES (1,1,'root','Runtime',NULL,NULL,1),(2,2,'other-root','Runtime',NULL,NULL,1),(3,3,'global-root','Runtime',NULL,NULL,1),(4,4,'root','Runtime',NULL,NULL,1);
    ").execute(&pool).await.unwrap();
    let content = serde_json::to_string(&vec![
        phoenix_core::domain::llm_types::ContentBlock::ToolUse {
            id: "call-1".into(),
            name: "goal_report".into(),
            input: serde_json::json!({}),
        },
    ])
    .unwrap();
    sqlx::query("UPDATE messages SET content=? WHERE message_type='agent'")
        .bind(content)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("schema.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, path.to_string_lossy().into_owned());
    (dir, db)
}

fn user() -> GoalActor {
    GoalActor::UserApi {
        transcript_id: t("root"),
        message_id: t("user-input"),
    }
}

fn generated() -> GoalActor {
    GoalActor::GeneratedHost {
        action_id: t("host-application"),
    }
}

fn agent(coordinator: bool) -> GoalActor {
    let source = GoalAgentSource {
        product_conversation_id: p(if coordinator { "global" } else { "owner" }),
        transcript_id: t(if coordinator { "global-root" } else { "root" }),
        execution: GoalExecution::parse(
            if coordinator { 3 } else { 1 },
            if coordinator { 3 } else { 1 },
        )
        .unwrap(),
        message_id: t(if coordinator {
            "global-call"
        } else {
            "agent-call"
        }),
        tool_use_id: t("call-1"),
    };
    if coordinator {
        GoalActor::Coordinator(source)
    } else {
        GoalActor::OwningAgent(source)
    }
}

fn create() -> GoalMutation {
    GoalMutation {
        owner: p("owner"),
        expected_version: GoalVersion::INITIAL,
        action_id: t("create"),
        attribution: GoalAttribution {
            requested_by: user(),
            applied_by: generated(),
        },
        change: GoalChange::Create {
            goal_id: GoalId::parse("goal-1").unwrap(),
            objective: t("prepare only"),
        },
    }
}

fn edit(snapshot: &GoalSnapshot, actor: GoalActor, id: &str) -> GoalMutation {
    GoalMutation {
        owner: snapshot.owner.clone(),
        expected_version: snapshot.version,
        action_id: t(id),
        attribution: GoalAttribution {
            requested_by: actor,
            applied_by: generated(),
        },
        change: GoalChange::EditObjective {
            goal_id: snapshot.goal_id.clone(),
            objective: t(id),
        },
    }
}

fn binding(snapshot: &GoalSnapshot) -> GoalExecutionBinding {
    GoalExecutionBinding {
        owner: snapshot.owner.clone(),
        execution: GoalExecution::parse(1, 1).unwrap(),
        observation: GoalAdmissionObservation::Goal(snapshot.fence()),
    }
}

async fn bind(db: &Database, binding: &GoalExecutionBinding) -> Result<()> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    bind_goal_at_admission_tx(&mut tx, binding).await?;
    tx.commit().await?;
    Ok(())
}

fn completion(snapshot: &GoalSnapshot, binding: GoalExecutionBinding) -> GoalMutation {
    GoalMutation {
        owner: snapshot.owner.clone(),
        expected_version: snapshot.version,
        action_id: t("completion"),
        attribution: GoalAttribution {
            requested_by: agent(false),
            applied_by: generated(),
        },
        change: GoalChange::Report {
            binding,
            disposition: GoalDisposition::Complete(t("prepared")),
        },
    }
}

#[tokio::test]
async fn normalized_history_roundtrips_all_actors_and_exact_retries() {
    let (_dir, db) = fixture().await;
    let request = create();
    let first = db.mutate_conversation_goal(&request, at()).await.unwrap();
    assert_eq!(
        db.mutate_conversation_goal(&request, GoalTimestampUs::parse(99).unwrap())
            .await
            .unwrap(),
        first
    );
    let second = db
        .mutate_conversation_goal(&edit(&first.snapshot, agent(false), "owner-edit"), at())
        .await
        .unwrap();
    let third = db
        .mutate_conversation_goal(&edit(&second.snapshot, agent(true), "global-edit"), at())
        .await
        .unwrap();
    let history = db
        .conversation_goal_history(&p("owner"), GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history, vec![first, second, third.clone()]);
    assert_eq!(
        db.conversation_goal(&p("owner")).await.unwrap(),
        Some(third.snapshot)
    );
    let mut conflict = request;
    conflict.change = GoalChange::Create {
        goal_id: GoalId::parse("goal-1").unwrap(),
        objective: t("different payload"),
    };
    assert!(matches!(
        db.mutate_conversation_goal(&conflict, at()).await,
        Err(GoalDbError::RetryConflict)
    ));
}

#[tokio::test]
async fn all_actor_writes_compete_on_observed_version() {
    let (_dir, db) = fixture().await;
    let first = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    let ui = edit(&first, user(), "ui-change");
    let global = edit(&first, agent(true), "global-change");
    let (a, b) = tokio::join!(
        db.mutate_conversation_goal(&ui, at()),
        db.mutate_conversation_goal(&global, at())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let error = if a.is_err() {
        a.unwrap_err()
    } else {
        b.unwrap_err()
    };
    assert!(matches!(error, GoalDbError::Domain(GoalError::Conflict)));
    assert_eq!(
        db.conversation_goal_history(&p("owner"), GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        2
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_goal_actors")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 4);
}

#[tokio::test]
async fn wrong_owner_role_and_fabricated_user_channel_roll_back_every_row() {
    let (_dir, db) = fixture().await;
    for actor in [
        GoalActor::Coordinator(match agent(false) {
            GoalActor::OwningAgent(s) => s,
            _ => unreachable!(),
        }),
        GoalActor::UserApi {
            transcript_id: t("root"),
            message_id: t("agent-call"),
        },
        GoalActor::OwningAgent(match agent(true) {
            GoalActor::Coordinator(s) => s,
            _ => unreachable!(),
        }),
    ] {
        let mut request = create();
        request.attribution.requested_by = actor;
        assert!(db.mutate_conversation_goal(&request, at()).await.is_err());
        assert!(db.conversation_goal(&p("owner")).await.unwrap().is_none());
        for table in [
            "conversation_goal_ids",
            "conversation_goal_actors",
            "conversation_goal_history",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(count, 0);
        }
    }
    let mut global_owner = create();
    global_owner.owner = p("global");
    assert!(db
        .mutate_conversation_goal(&global_owner, at())
        .await
        .is_err());
}

#[tokio::test]
async fn admitted_binding_is_immutable_and_late_report_cannot_rebind_to_edit() {
    let (_dir, db) = fixture().await;
    let first = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    let captured = binding(&first);
    bind(&db, &captured).await.unwrap();
    let second = db
        .mutate_conversation_goal(&edit(&first, user(), "new-objective"), at())
        .await
        .unwrap()
        .snapshot;
    let newer_binding = binding(&second);
    assert!(matches!(
        bind(&db, &newer_binding).await,
        Err(GoalDbError::BindingConflict)
    ));
    bind(&db, &captured).await.unwrap();
    let mut stale = completion(&first, captured);
    stale.expected_version = second.version;
    assert!(matches!(
        db.mutate_conversation_goal(&stale, at()).await,
        Err(GoalDbError::Domain(GoalError::Conflict))
    ));
    let forged = completion(&second, newer_binding);
    assert!(matches!(
        db.mutate_conversation_goal(&forged, at()).await,
        Err(GoalDbError::BindingConflict)
    ));
    assert_eq!(
        db.conversation_goal(&p("owner")).await.unwrap(),
        Some(second)
    );
}

#[tokio::test]
async fn report_completion_retains_binding_and_history_across_restart() {
    let (dir, db) = fixture().await;
    let first = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    let captured = binding(&first);
    bind(&db, &captured).await.unwrap();
    let report = completion(&first, captured.clone());
    let done = db.mutate_conversation_goal(&report, at()).await.unwrap();
    db.pool().close().await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(dir.path().join("goals.db"))
                .foreign_keys(true),
        )
        .await
        .unwrap();
    let reopened = Database::from_pool_for_tests(pool, "goals.db".into());
    assert_eq!(
        reopened
            .mutate_conversation_goal(&report, at())
            .await
            .unwrap(),
        done
    );
    let mut tx = reopened.pool().begin().await.unwrap();
    assert_eq!(binding_tx(&mut tx, 1).await.unwrap(), Some(captured));
    tx.commit().await.unwrap();
    assert!(matches!(done.snapshot.state, GoalState::Complete(_)));
}

#[tokio::test]
async fn no_goal_admission_cannot_acquire_a_later_goal() {
    let (_dir, db) = fixture().await;
    assert!(db.conversation_goal(&p("owner")).await.unwrap().is_none());
    let no_goal = GoalExecutionBinding {
        owner: p("owner"),
        execution: GoalExecution::parse(1, 1).unwrap(),
        observation: GoalAdmissionObservation::NoGoal {
            observed_version: GoalVersion::INITIAL,
        },
    };
    bind(&db, &no_goal).await.unwrap();
    let first = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    assert!(matches!(
        bind(&db, &binding(&first)).await,
        Err(GoalDbError::BindingConflict)
    ));
    bind(&db, &no_goal).await.unwrap();
}

#[tokio::test]
async fn binding_rejects_cross_owner_and_already_materialized_work() {
    let (_dir, db) = fixture().await;
    let first = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    let mut wrong = binding(&first);
    wrong.execution = GoalExecution::parse(2, 2).unwrap();
    assert!(bind(&db, &wrong).await.is_err());
    sqlx::query("UPDATE durable_turns SET canonical_message_id='user-input' WHERE turn_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(bind(&db, &binding(&first)).await.is_err());
}

#[tokio::test]
async fn history_and_binding_rows_cannot_be_rewritten_or_deleted() {
    let (_dir, db) = fixture().await;
    let first = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    bind(&db, &binding(&first)).await.unwrap();
    for sql in [
        "UPDATE conversation_goal_history SET objective='overwritten'",
        "DELETE FROM conversation_goal_history",
        "UPDATE conversation_goal_actors SET kind='generated_host'",
        "DELETE FROM conversation_goal_actors",
        "UPDATE conversation_goal_bindings SET observed_version=NULL,goal_id=NULL,objective_revision=NULL",
        "DELETE FROM conversation_goal_bindings",
    ] {
        assert!(sqlx::query(sql).execute(db.pool()).await.is_err(), "{sql}");
    }
}

#[tokio::test]
async fn staged_schema_applies_after_real_migrations_without_rewriting_existing_data() {
    let db = Database::open_in_memory().await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM product_conversations")
        .fetch_one(db.pool())
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("schema.sql"))
        .execute(db.pool())
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM product_conversations")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(before, after);
    assert!(db
        .conversation_goal(&p("not-present"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn agent_locator_must_name_a_real_assistant_tool_call_for_both_actor_roles() {
    let (_dir, db) = fixture().await;
    for requested in [true, false] {
        for (message, tool) in [("user-input", "call-1"), ("agent-call", "invented-call")] {
            let mut actor = agent(false);
            if let GoalActor::OwningAgent(source) = &mut actor {
                source.message_id = t(message);
                source.tool_use_id = t(tool);
            }
            let mut mutation = create();
            if requested {
                mutation.attribution.requested_by = actor;
            } else {
                mutation.attribution.applied_by = actor;
            }
            assert!(db.mutate_conversation_goal(&mutation, at()).await.is_err());
            for table in [
                "conversation_goal_ids",
                "conversation_goal_actors",
                "conversation_goal_history",
            ] {
                let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
                assert_eq!(count, 0);
            }
        }
    }
}

#[tokio::test]
async fn failed_history_insert_rolls_back_both_actor_rows() {
    let (_dir, db) = fixture().await;
    sqlx::raw_sql("CREATE TRIGGER fail_goal_history BEFORE INSERT ON conversation_goal_history BEGIN SELECT RAISE(ABORT,'injected history failure'); END;")
        .execute(db.pool()).await.unwrap();
    assert!(db.mutate_conversation_goal(&create(), at()).await.is_err());
    for table in [
        "conversation_goal_ids",
        "conversation_goal_actors",
        "conversation_goal_history",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn cleared_goal_does_not_allow_id_or_version_aba() {
    let (_dir, db) = fixture().await;
    let first = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    let clear = GoalMutation {
        owner: first.owner.clone(),
        expected_version: first.version,
        action_id: t("clear"),
        attribution: GoalAttribution {
            requested_by: user(),
            applied_by: generated(),
        },
        change: GoalChange::SetDisposition {
            goal_id: first.goal_id.clone(),
            disposition: GoalDisposition::Cleared(t("unfinished archive")),
        },
    };
    let cleared = db
        .mutate_conversation_goal(&clear, at())
        .await
        .unwrap()
        .snapshot;
    let mut reused = create();
    reused.action_id = t("reuse-id");
    reused.expected_version = cleared.version;
    assert!(db.mutate_conversation_goal(&reused, at()).await.is_err());
    let mut replacement = reused;
    replacement.action_id = t("replace");
    replacement.change = GoalChange::Create {
        goal_id: GoalId::parse("goal-2").unwrap(),
        objective: t("new objective"),
    };
    let second = db
        .mutate_conversation_goal(&replacement, at())
        .await
        .unwrap()
        .snapshot;
    assert_eq!(second.version.get(), 3);
    assert_eq!(second.objective_revision.get(), 1);
    let mut stale = edit(&first, agent(true), "late-global-edit");
    assert!(matches!(
        db.mutate_conversation_goal(&stale, at()).await,
        Err(GoalDbError::Domain(GoalError::Conflict))
    ));
    stale.expected_version = second.version;
    assert!(matches!(
        db.mutate_conversation_goal(&stale, at()).await,
        Err(GoalDbError::Domain(GoalError::Conflict))
    ));
}

#[tokio::test]
async fn timestamp_constraints_reject_text_and_negative_integers() {
    let (_dir, db) = fixture().await;
    db.mutate_conversation_goal(&create(), at()).await.unwrap();
    for timestamp in ["'not-an-integer'", "-1", "1.5"] {
        let sql = format!("INSERT INTO conversation_goal_history (owner_id,version,goal_id,objective_revision,objective,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) SELECT owner_id,2,goal_id,2,'edit',state,reason,'timestamp-test','edit',NULL,requested_actor_id,applied_actor_id,{timestamp} FROM conversation_goal_history WHERE version=1");
        assert!(sqlx::query(&sql).execute(db.pool()).await.is_err());
    }
    assert_eq!(
        db.conversation_goal_history(&p("owner"), GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn raw_report_row_cannot_reference_another_owner_or_wrong_version() {
    let (_dir, db) = fixture().await;
    let first = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    bind(&db, &binding(&first)).await.unwrap();
    let other = GoalExecutionBinding {
        owner: p("other"),
        execution: GoalExecution::parse(2, 2).unwrap(),
        observation: GoalAdmissionObservation::NoGoal {
            observed_version: GoalVersion::INITIAL,
        },
    };
    bind(&db, &other).await.unwrap();
    for turn in [1, 2] {
        let sql = format!("INSERT INTO conversation_goal_history (owner_id,version,goal_id,objective_revision,objective,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) SELECT owner_id,2,goal_id,objective_revision,objective,'complete','done','raw-report','report',{turn},requested_actor_id,applied_actor_id,10 FROM conversation_goal_history WHERE version=1");
        // turn=1 also fails: the copied requester is UserApi, not the admitted owning agent.
        assert!(sqlx::query(&sql).execute(db.pool()).await.is_err());
    }
    let second = db
        .mutate_conversation_goal(&edit(&first, agent(false), "edit-before-report"), at())
        .await
        .unwrap()
        .snapshot;
    let sql = "INSERT INTO conversation_goal_history (owner_id,version,goal_id,objective_revision,objective,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) SELECT owner_id,3,goal_id,objective_revision,objective,'complete','done','raw-late-report','report',1,requested_actor_id,applied_actor_id,10 FROM conversation_goal_history WHERE version=2";
    assert!(sqlx::query(sql).execute(db.pool()).await.is_err());
    assert_eq!(
        db.conversation_goal(&p("owner")).await.unwrap(),
        Some(second)
    );
}

async fn full_substrate() -> (Database, ProductConversationId) {
    let db = Database::open_in_memory().await.unwrap();
    sqlx::raw_sql(include_str!("schema.sql"))
        .execute(db.pool())
        .await
        .unwrap();
    let conversation = db
        .create_conversation("full-root", "full-goal", "/tmp", true, None, None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO messages (conversation_id,message_id,sequence_id,message_type,content,created_at,origin_kind) VALUES ('full-root','full-input',1,'user',?,?,'user_api')")
        .bind(serde_json::json!({"text":"prepare only","llm_text":null,"is_meta":false}).to_string())
        .bind("2026-10-19T00:00:00Z").execute(db.pool()).await.unwrap();
    (db, conversation.product_conversation_id)
}

#[tokio::test]
async fn real_migrations_user_goal_clear_and_supported_hard_delete_do_not_pin_aggregate() {
    let (db, owner) = full_substrate().await;
    let mut request = create();
    request.owner = owner.clone();
    request.attribution.requested_by = GoalActor::UserApi {
        transcript_id: t("full-root"),
        message_id: t("full-input"),
    };
    let first = db
        .mutate_conversation_goal(&request, at())
        .await
        .unwrap()
        .snapshot;
    let clear = GoalMutation {
        owner: owner.clone(),
        expected_version: first.version,
        action_id: t("clear"),
        attribution: request.attribution,
        change: GoalChange::SetDisposition {
            goal_id: first.goal_id,
            disposition: GoalDisposition::Cleared(t("unfinished")),
        },
    };
    db.mutate_conversation_goal(&clear, at()).await.unwrap();
    db.delete_conversation("full-root").await.unwrap();
    for table in [
        "conversation_goal_ids",
        "conversation_goal_history",
        "conversation_goal_actors",
    ] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE owner_id=?"))
                .bind(owner.as_str())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
}

#[tokio::test]
async fn real_migrations_no_goal_binding_does_not_pin_supported_hard_delete() {
    let (db, owner) = full_substrate().await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query("INSERT INTO workflows (workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'direct_turn',1,1,1,0,0,'Active','direct_turn',1,X'00',0,0)")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO durable_turns (turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation) VALUES (1,1,'full-root','full-turn','fixture',X'00','Runtime',0,1)")
        .execute(&mut *tx).await.unwrap();
    let observation = GoalExecutionBinding {
        owner: owner.clone(),
        execution: GoalExecution::parse(1, 1).unwrap(),
        observation: GoalAdmissionObservation::NoGoal {
            observed_version: GoalVersion::INITIAL,
        },
    };
    bind_goal_at_admission_tx(&mut tx, &observation)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.delete_conversation("full-root").await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM conversation_goal_bindings WHERE owner_id=?")
            .bind(owner.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
}
