use super::*;
use phoenix_core::domain::coordinator_authority::MAX_AUTHORITY_BYTES;
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::llm_language::LlmLanguage;

async fn coordinator(db: &Database) -> String {
    db.get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap()
        .id
}

fn instructions(text: &str) -> CoordinatorAuthority {
    CoordinatorAuthority::try_from(text.to_owned()).unwrap()
}

#[tokio::test]
async fn coordinator_authority_replaces_clears_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("authority.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::run_pending_migrations(db.pool()).await.unwrap();
    let source = coordinator(&db).await;
    assert!(db.coordinator_authority().await.unwrap().is_empty());
    for text in [
        "  Preserve source.\nAllow unused cache cleanup. 🐦\n",
        "Replacement",
    ] {
        let value = instructions(text);
        let written = db
            .set_coordinator_authority(&source, &value)
            .await
            .established()
            .unwrap()
            .unwrap();
        assert_eq!(written, value);
        assert_eq!(db.read_coordinator_authority(&source).await.unwrap(), value);
    }
    db.pool.close().await;
    let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::run_pending_migrations(reopened.pool())
        .await
        .unwrap();
    assert_eq!(
        reopened.coordinator_authority().await.unwrap().as_str(),
        "Replacement"
    );
    reopened
        .set_coordinator_authority(&source, &CoordinatorAuthority::default())
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(reopened.coordinator_authority().await.unwrap().is_empty());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM app_settings WHERE key = ?1")
        .bind(SETTING_KEY)
        .fetch_one(reopened.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn coordinator_authority_checks_current_source_and_preserves_text_across_rotation() {
    let db = Database::open_in_memory().await.unwrap();
    let source = coordinator(&db).await;
    let text = instructions("Standing permission");
    db.set_coordinator_authority(&source, &text)
        .await
        .established()
        .unwrap()
        .unwrap();
    let ordinary = db
        .create_conversation("authority-ordinary", "ordinary", "/tmp", true, None, None)
        .await
        .unwrap();
    let child = db
        .create_subagent_conversation(
            "authority-child",
            "authority-child",
            "/tmp",
            &ordinary.id,
            "offline-model",
            &ordinary.conv_mode,
            LlmLanguage::default(),
            ordinary.attached_work_scope_id.as_ref(),
            crate::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
    db.update_conversation_state(
        &source,
        &ConvState::ContextExhausted {
            summary: "rotate Coordinator".into(),
        },
    )
    .await
    .unwrap();
    let successor = match db.continue_conversation(&source).await.unwrap() {
        crate::ContinueOutcome::Created(successor) => successor,
        other @ (crate::ContinueOutcome::AlreadyContinued(_)
        | crate::ContinueOutcome::ParentNotContextExhausted { .. }) => {
            panic!("expected successor: {other:?}")
        }
    };
    for rejected in [
        source.as_str(),
        ordinary.id.as_str(),
        child.id.as_str(),
        "missing",
    ] {
        assert!(matches!(
            db.read_coordinator_authority(rejected).await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
        assert!(matches!(
            db.set_coordinator_authority(rejected, &CoordinatorAuthority::default())
                .await
                .established()
                .unwrap(),
            Err(DbError::CurrentCoordinatorRequired)
        ));
    }
    assert_eq!(
        db.read_coordinator_authority(&successor.id).await.unwrap(),
        text
    );
}

#[tokio::test]
async fn coordinator_authority_invalid_stored_text_and_read_failure_are_visible() {
    let db = Database::open_in_memory().await.unwrap();
    let source = coordinator(&db).await;
    db.set_app_setting(SETTING_KEY, &"🐦".repeat(MAX_AUTHORITY_BYTES / 4 + 1))
        .await
        .unwrap();
    assert!(matches!(
        db.coordinator_authority().await,
        Err(DbError::Serialization(_))
    ));
    assert!(db.read_coordinator_authority(&source).await.is_err());
    sqlx::query("DROP TABLE app_settings")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db.coordinator_authority().await.is_err());
}

#[tokio::test]
async fn coordinator_authority_failed_write_rolls_back_and_lost_ack_is_unclassified() {
    let db = Database::open_in_memory().await.unwrap();
    let source = coordinator(&db).await;
    let previous = instructions("Keep this");
    db.set_coordinator_authority(&source, &previous)
        .await
        .established()
        .unwrap()
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER refuse_authority BEFORE UPDATE ON app_settings \
         WHEN NEW.key = 'coordinator_authority' BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(db
        .set_coordinator_authority(&source, &instructions("Replacement"))
        .await
        .established()
        .unwrap()
        .is_err());
    assert_eq!(db.coordinator_authority().await.unwrap(), previous);
    sqlx::query("DROP TRIGGER refuse_authority")
        .execute(db.pool())
        .await
        .unwrap();
    let committed = instructions("Committed without acknowledgement");
    assert!(matches!(
        db.set_coordinator_authority_with_commit(
            &source,
            &committed,
            AuthorityCommit::LoseAcknowledgement,
        )
        .await,
        LocalAuthorityResult::DurableFactUnclassified
    ));
    assert_eq!(db.coordinator_authority().await.unwrap(), committed);
}
