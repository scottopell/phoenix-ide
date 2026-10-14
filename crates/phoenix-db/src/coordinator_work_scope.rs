use super::{Database, DbError, DbResult};
use chrono::Utc;
use phoenix_core::work_scope::{
    AuthorityKind, EnvironmentContext, WorkScopeEnvironmentRecord, WorkScopeId,
};
use sqlx::{Row, Sqlite, Transaction};

impl Database {
    /// Read the instance-owned home without creating a Coordinator transcript.
    ///
    /// # Errors
    /// Returns an error for invalid ownership or database failure.
    pub async fn coordinator_work_scope(&self) -> DbResult<Option<WorkScopeEnvironmentRecord>> {
        let mut tx = self.pool.begin().await?;
        let record = Self::coordinator_work_scope_tx(&mut tx).await?;
        tx.commit().await?;
        Ok(record)
    }

    pub(crate) async fn coordinator_work_scope_tx(
        tx: &mut Transaction<'_, Sqlite>,
    ) -> DbResult<Option<WorkScopeEnvironmentRecord>> {
        let inconsistent: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM work_scopes scope
                WHERE scope.environment_kind = 'coordinator_home'
                  AND NOT EXISTS (SELECT 1 FROM coordinator_work_scope owner WHERE owner.work_scope_id = scope.id)
             ) OR EXISTS (
                SELECT 1 FROM coordinator_work_scope owner
                JOIN product_conversation_work_scopes ordinary ON ordinary.work_scope_id = owner.work_scope_id
             ) OR EXISTS (
                SELECT 1 FROM coordinator_work_scope owner
                JOIN conversations c ON c.work_scope_id = owner.work_scope_id
                WHERE c.runtime_role <> 'coordinator'
             )",
        ).fetch_one(&mut **tx).await?;
        if inconsistent {
            return Err(DbError::CoordinatorWorkScope(
                "Coordinator home has conflicting ownership".to_string(),
            ));
        }
        let row = sqlx::query(
            "SELECT owner.work_scope_id, scope.cwd, scope.authority_kind, scope.lifecycle, scope.environment_kind
             FROM coordinator_work_scope owner LEFT JOIN work_scopes scope ON scope.id = owner.work_scope_id
             WHERE owner.singleton = 1",
        ).fetch_optional(&mut **tx).await?;
        row.map(|row| {
            if row
                .try_get::<Option<String>, _>("authority_kind")?
                .as_deref()
                != Some("work")
                || row.try_get::<Option<String>, _>("lifecycle")?.as_deref() != Some("active")
                || row
                    .try_get::<Option<String>, _>("environment_kind")?
                    .as_deref()
                    != Some("coordinator_home")
            {
                return Err(DbError::CoordinatorWorkScope(
                    "persisted Coordinator ownership is invalid".to_string(),
                ));
            }
            let cwd = row
                .try_get::<Option<String>, _>("cwd")?
                .filter(|cwd| !cwd.trim().is_empty())
                .ok_or_else(|| {
                    DbError::CoordinatorWorkScope(
                        "persisted Coordinator home has no cwd".to_string(),
                    )
                })?;
            Ok(WorkScopeEnvironmentRecord {
                work_scope_id: WorkScopeId::parse(row.try_get::<String, _>("work_scope_id")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                context: EnvironmentContext::CoordinatorHome { cwd },
            })
        })
        .transpose()
    }

    /// Initialize the immutable instance-owned home and reconcile existing Coordinator transcripts.
    /// Filesystem validation belongs to the startup caller; this method does not create directories.
    ///
    /// # Errors
    /// Rejects empty cwd, a different persisted cwd, or a database failure.
    pub async fn initialize_coordinator_work_scope(
        &self,
        cwd: &str,
    ) -> DbResult<WorkScopeEnvironmentRecord> {
        if cwd.trim().is_empty() {
            return Err(DbError::CoordinatorWorkScope(
                "Coordinator home cwd must not be empty".to_string(),
            ));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(record) = Self::coordinator_work_scope_tx(&mut tx).await? {
            if record.context
                != (EnvironmentContext::CoordinatorHome {
                    cwd: cwd.to_string(),
                })
            {
                return Err(DbError::CoordinatorWorkScope(
                    "configured Coordinator home differs from persisted cwd; refusing to rebind"
                        .to_string(),
                ));
            }
            Self::reconcile_coordinator_work_scope_tx(&mut tx, &record.work_scope_id).await?;
            tx.commit().await?;
            return Ok(record);
        }
        let scope_id = WorkScopeId::new();
        Self::insert_work_scope_tx(
            &mut tx,
            &scope_id,
            AuthorityKind::Work,
            EnvironmentContext::CoordinatorHome {
                cwd: cwd.to_string(),
            },
            &Utc::now().to_rfc3339(),
        )
        .await?;
        let record = Self::coordinator_work_scope_tx(&mut tx)
            .await?
            .ok_or_else(|| {
                DbError::CoordinatorWorkScope(
                    "Coordinator ownership was not initialized".to_string(),
                )
            })?;
        tx.commit().await?;
        Ok(record)
    }

    pub(crate) async fn reconcile_coordinator_work_scope_tx(
        tx: &mut Transaction<'_, Sqlite>,
        scope_id: &WorkScopeId,
    ) -> DbResult<()> {
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE runtime_role = 'coordinator' AND work_scope_id IS NOT ?1")
            .bind(scope_id.as_str()).execute(&mut **tx).await?;
        Ok(())
    }

    pub(crate) async fn reject_coordinator_conversation_mutation_conn(
        connection: &mut sqlx::SqliteConnection,
        conversation_id: &str,
    ) -> DbResult<()> {
        let coordinator: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1 AND runtime_role = 'coordinator')")
            .bind(conversation_id).fetch_one(connection).await?;
        if coordinator {
            return Err(DbError::CoordinatorWorkScope(
                "Coordinator transcript is outside ordinary mutation and cleanup".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn reject_coordinator_scope_mutation_tx(
        tx: &mut Transaction<'_, Sqlite>,
        scope_id: &WorkScopeId,
    ) -> DbResult<()> {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM coordinator_work_scope WHERE work_scope_id = ?1)",
        )
        .bind(scope_id.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if owned {
            return Err(DbError::CoordinatorWorkScope(
                "Coordinator-owned WorkScope is outside ordinary mutation and cleanup".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContinueOutcome, ConversationCreationMetadataUpdate};
    use phoenix_core::domain::db_schema::ConvMode;
    use phoenix_core::domain::sm_state::ConvState;
    use phoenix_core::llm_language::LlmLanguage;
    use phoenix_core::work_scope::{
        WorkScopeLifecycle, WorkScopeRetirementBlocker, WorkScopeRetirementOutcome,
        WorkScopeRetirementPrecondition,
    };

    async fn coordinator(db: &Database) -> crate::Conversation {
        db.get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn home_exists_without_transcripts_or_ordinary_work() {
        let db = Database::open_in_memory().await.unwrap();
        assert!(db.coordinator_work_scope().await.unwrap().is_none());
        assert!(matches!(
            db.initialize_coordinator_work_scope(" \t").await,
            Err(DbError::CoordinatorWorkScope(_))
        ));
        let home = db
            .initialize_coordinator_work_scope("/owned/coordinator")
            .await
            .unwrap();
        assert_eq!(
            home.context,
            EnvironmentContext::CoordinatorHome {
                cwd: "/owned/coordinator".into()
            }
        );
        assert_eq!(
            db.coordinator_work_scope().await.unwrap(),
            Some(home.clone())
        );
        assert_eq!(
            db.initialize_coordinator_work_scope("/owned/coordinator")
                .await
                .unwrap(),
            home
        );
        assert!(matches!(
            db.initialize_coordinator_work_scope("/different/coordinator")
                .await,
            Err(DbError::CoordinatorWorkScope(_))
        ));
        for table in [
            "conversations",
            "product_conversations",
            "product_conversation_work_scopes",
        ] {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(count, 0);
        }
        let conv = coordinator(&db).await;
        assert_eq!(conv.attached_work_scope_id, Some(home.work_scope_id));
        assert_eq!(conv.cwd, "/owned/coordinator");
        assert_eq!(
            db.get_conversation_work_scope_context(&conv.id)
                .await
                .unwrap(),
            (
                AuthorityKind::Work,
                WorkScopeLifecycle::Active,
                home.context
            )
        );
    }

    #[tokio::test]
    async fn initialization_reconciles_entire_existing_coordinator_chain() {
        let db = Database::open_in_memory().await.unwrap();
        let root = coordinator(&db).await;
        assert!(root.attached_work_scope_id.is_none());
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "accepted handoff".into(),
            },
        )
        .await
        .unwrap();
        let ContinueOutcome::Created(next) = db.continue_conversation(&root.id).await.unwrap()
        else {
            panic!("expected continuation")
        };
        let home = db
            .initialize_coordinator_work_scope("/owned/coordinator")
            .await
            .unwrap();
        for id in [&root.id, &next.id] {
            let conv = db.get_conversation(id).await.unwrap();
            assert_eq!(
                conv.attached_work_scope_id.as_ref(),
                Some(&home.work_scope_id)
            );
            assert_eq!(conv.cwd, "/owned/coordinator");
        }
        assert_eq!(coordinator(&db).await.id, next.id);
        let mappings: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_conversation_work_scopes")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(mappings, 0);
    }

    #[tokio::test]
    async fn singleton_initialization_is_concurrent_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coordinator.db");
        let path = path.to_str().unwrap();
        let db = Database::open(path).await.unwrap();
        let second = Database::open(path).await.unwrap();
        let (first, other) = tokio::join!(
            db.initialize_coordinator_work_scope("/owned/coordinator"),
            second.initialize_coordinator_work_scope("/owned/coordinator")
        );
        let home = first.unwrap();
        assert_eq!(home, other.unwrap());
        let (a, b) = tokio::join!(coordinator(&db), coordinator(&second));
        assert_eq!(a.id, b.id);
        db.pool().close().await;
        second.pool().close().await;
        let reopened = Database::open(path).await.unwrap();
        assert_eq!(
            reopened.coordinator_work_scope().await.unwrap(),
            Some(home.clone())
        );
        assert_eq!(
            coordinator(&reopened).await.attached_work_scope_id,
            Some(home.work_scope_id)
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_scopes")
            .fetch_one(reopened.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn direct_mode_coordinator_continuation_never_allocates_ordinary_scope() {
        let db = Database::open_in_memory().await.unwrap();
        let home = db
            .initialize_coordinator_work_scope("/owned/coordinator")
            .await
            .unwrap();
        let root = coordinator(&db).await;
        sqlx::query("UPDATE conversations SET cm_kind = 'direct' WHERE id = ?1")
            .bind(&root.id)
            .execute(db.pool())
            .await
            .unwrap();
        for parent in [root.id.clone()] {
            db.update_conversation_state(
                &parent,
                &ConvState::ContextExhausted {
                    summary: "handoff".into(),
                },
            )
            .await
            .unwrap();
            let ContinueOutcome::Created(next) = db.continue_conversation(&parent).await.unwrap()
            else {
                panic!("expected continuation")
            };
            assert_eq!(
                next.attached_work_scope_id,
                Some(home.work_scope_id.clone())
            );
            assert_eq!(next.cwd, "/owned/coordinator");
            assert!(matches!(next.conv_mode, ConvMode::Direct));
            assert_eq!(coordinator(&db).await.id, next.id);
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_scopes")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn ordinary_mutation_cleanup_and_raw_sql_cannot_change_home() {
        let db = Database::open_in_memory().await.unwrap();
        let home = db
            .initialize_coordinator_work_scope("/owned/coordinator")
            .await
            .unwrap();
        let conv = coordinator(&db).await;
        assert_eq!(
            db.retire_work_scope(
                WorkScopeRetirementPrecondition::after_runtime_inventory_found_no_live_resource(
                    home.work_scope_id.clone()
                ),
                "ordinary cleanup"
            )
            .await
            .unwrap(),
            WorkScopeRetirementOutcome::Blocked(WorkScopeRetirementBlocker::CoordinatorOwned)
        );
        assert!(matches!(
            db.update_conversation_cwd_recovery_only(&conv.id, "/different")
                .await,
            Err(DbError::CoordinatorWorkScope(_))
        ));
        assert!(matches!(
            db.update_conversation_mode(&conv.id, &ConvMode::Direct)
                .await,
            Err(DbError::CoordinatorWorkScope(_))
        ));
        assert!(matches!(
            db.update_conversation_mode_and_cwd(&conv.id, &ConvMode::Direct, "/different")
                .await,
            Err(DbError::CoordinatorWorkScope(_))
        ));
        let update = ConversationCreationMetadataUpdate {
            slug: Some("must-not-change".into()),
            cwd: Some("/different".into()),
            title: None,
            project_id: None,
            desired_base_branch: None,
        };
        assert!(matches!(
            db.update_conversation_creation_metadata(&conv.id, &update)
                .await,
            Err(DbError::CoordinatorWorkScope(_))
        ));
        assert!(matches!(
            db.delete_conversation(&conv.id).await,
            Err(DbError::CoordinatorWorkScope(_))
        ));
        let ordinary = db
            .create_conversation("ordinary", "ordinary", "/ordinary", true, None, None)
            .await
            .unwrap();
        let ordinary_scope = ordinary.attached_work_scope_id.unwrap();
        for statement in [
            "UPDATE work_scopes SET cwd = '/different' WHERE id = ?1",
            "UPDATE work_scopes SET environment_kind = 'unowned_cwd' WHERE id = ?1",
            "UPDATE work_scopes SET authority_kind = 'restricted_explore' WHERE id = ?1",
            "UPDATE work_scopes SET lifecycle = 'retired', retired_at = 'now' WHERE id = ?1",
            "UPDATE work_scopes SET worktree_id = 'fake', worktree_fingerprint = 'fake' WHERE id = ?1",
            "UPDATE work_scopes SET id = 'replacement' WHERE id = ?1",
            "DELETE FROM work_scopes WHERE id = ?1",
            "DELETE FROM coordinator_work_scope WHERE work_scope_id = ?1",
            "UPDATE coordinator_work_scope SET work_scope_id = 'replacement' WHERE work_scope_id = ?1",
            "UPDATE conversations SET work_scope_id = NULL WHERE work_scope_id = ?1",
            "UPDATE conversations SET work_scope_id = ?1 WHERE id = 'ordinary'",
            "DELETE FROM conversations WHERE work_scope_id = ?1",
            "INSERT INTO product_conversation_work_scopes (work_scope_id, product_conversation_id) SELECT ?1, product_conversation_id FROM conversations WHERE id = 'ordinary'",
        ] {
            assert!(sqlx::query(statement).bind(home.work_scope_id.as_str()).execute(db.pool()).await.is_err(), "{statement}");
        }
        assert!(
            sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = ?2")
                .bind(ordinary_scope.as_str())
                .bind(&conv.id)
                .execute(db.pool())
                .await
                .is_err()
        );
        assert!(sqlx::query("INSERT INTO work_scopes (id, authority_kind, environment_kind, cwd, created_at, updated_at) VALUES ('second-home', 'work', 'coordinator_home', '/second', 'now', 'now')").execute(db.pool()).await.is_err());
        db.delete_conversation(&ordinary.id).await.unwrap();
        assert_eq!(db.coordinator_work_scope().await.unwrap(), Some(home));
        assert_eq!(db.get_conversation(&conv.id).await.unwrap().slug, conv.slug);
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .is_empty());
    }
}
