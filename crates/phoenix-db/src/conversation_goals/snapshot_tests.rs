use super::*;
use tokio::sync::oneshot;

pub(super) struct HistoryReadCut {
    pub(super) owner_checked: oneshot::Sender<()>,
    pub(super) resume: oneshot::Receiver<()>,
}

impl HistoryReadCut {
    pub(super) async fn pause(self) {
        self.owner_checked.send(()).expect("history owner checked");
        self.resume.await.expect("resume history after hard delete");
    }
}

async fn file_backed_pair() -> (tempfile::TempDir, Database, Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("goal-history-snapshot.sqlite");
    let reader = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::migrations::run_pending_migrations(reader.pool())
        .await
        .unwrap();
    let writer = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::migrations::run_pending_migrations(writer.pool())
        .await
        .unwrap();
    (dir, reader, writer)
}

async fn seed_history(db: &Database, owner: &ProductConversationId) {
    sqlx::query("INSERT INTO conversation_goal_ids(goal_id,objective,owner_id) VALUES ('snapshot-goal','retained objective',?)")
        .bind(owner.as_str()).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_actors(actor_id,owner_id,kind,host_action_id) VALUES (1,?,'generated_host','snapshot-create')")
        .bind(owner.as_str()).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_history(owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,1,'snapshot-goal','underway',NULL,'snapshot-create','create',NULL,1,1,1234)")
        .bind(owner.as_str()).execute(db.pool()).await.unwrap();
}

fn assert_owner_not_found(result: Result<Vec<GoalHistoryEntry>>, owner: &ProductConversationId) {
    assert!(matches!(
        result,
        Err(GoalDbError::Admission(crate::DbError::ConversationNotFound(id)))
            if id == owner.as_str()
    ));
}

#[tokio::test]
async fn history_rejects_missing_and_non_ordinary_owners_but_accepts_empty_ordinary_history() {
    let db = Database::open_in_memory().await.unwrap();
    let ordinary = db
        .create_conversation(
            "empty-goal-history",
            "empty-goal-history",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(db
        .conversation_goal_history(&ordinary.product_conversation_id, GoalVersion::INITIAL)
        .await
        .unwrap()
        .is_empty());

    let missing = ProductConversationId::parse("missing-goal-history-owner").unwrap();
    assert_owner_not_found(
        db.conversation_goal_history(&missing, GoalVersion::INITIAL)
            .await,
        &missing,
    );
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    assert_owner_not_found(
        db.conversation_goal_history(&coordinator.product_conversation_id, GoalVersion::INITIAL)
            .await,
        &coordinator.product_conversation_id,
    );
}

#[tokio::test]
async fn history_keeps_owner_and_facts_in_one_snapshot_across_hard_delete() {
    let (_dir, reader, writer) = file_backed_pair().await;
    let conversation = reader
        .create_conversation("snapshot-owner", "snapshot-owner", "/tmp", true, None, None)
        .await
        .unwrap();
    let owner = conversation.product_conversation_id;
    seed_history(&reader, &owner).await;
    let expected = reader
        .conversation_goal_history(&owner, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(expected.len(), 1);
    assert_eq!(
        expected[0].snapshot().objective().as_str(),
        "retained objective"
    );
    assert_eq!(expected[0].recorded_at_us().get(), 1234);

    let (owner_checked, wait_for_owner) = oneshot::channel();
    let (resume_history, resume) = oneshot::channel();
    let read = reader.conversation_goal_history_with_cut(
        &owner,
        GoalVersion::INITIAL,
        Some(HistoryReadCut {
            owner_checked,
            resume,
        }),
    );
    let delete = async {
        wait_for_owner.await.expect("reader reached owner check");
        writer.delete_conversation(&conversation.id).await.unwrap();
        let remaining: (i64, i64, i64) = sqlx::query_as(
            "SELECT
                (SELECT COUNT(*) FROM conversation_goal_ids WHERE owner_id=?1),
                (SELECT COUNT(*) FROM conversation_goal_history WHERE owner_id=?1),
                (SELECT COUNT(*) FROM conversation_goal_actors WHERE owner_id=?1)",
        )
        .bind(owner.as_str())
        .fetch_one(writer.pool())
        .await
        .unwrap();
        assert_eq!(remaining, (0, 0, 0));
        assert_owner_not_found(
            writer
                .conversation_goal_history(&owner, GoalVersion::INITIAL)
                .await,
            &owner,
        );
        resume_history.send(()).expect("resume pinned reader");
    };
    let (history, ()) = tokio::join!(read, delete);
    assert_eq!(history.unwrap(), expected);
    assert_owner_not_found(
        reader
            .conversation_goal_history(&owner, GoalVersion::INITIAL)
            .await,
        &owner,
    );
}
