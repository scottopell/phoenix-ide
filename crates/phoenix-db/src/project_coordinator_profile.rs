use phoenix_core::domain::product_conversation::{
    ProductConversationId, ProjectCoordinatorProfile, ProjectCoordinatorProfileWriteError,
    PROJECT_COORDINATOR_CHARTER_MAX_BYTES,
};
use sqlx::Row;

use crate::{Database, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectCoordinatorProfileWriteOutcome {
    Saved(ProjectCoordinatorProfile),
    Disabled,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectCoordinatorProfileWriteDbError {
    #[error(transparent)]
    Domain(#[from] ProjectCoordinatorProfileWriteError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Project Coordinator profile commit outcome is unclassifiable")]
    AmbiguousCommit,
}

fn validate_charter(charter: &str) -> Result<(), ProjectCoordinatorProfileWriteError> {
    if charter.as_bytes().contains(&0) || charter.len() > PROJECT_COORDINATOR_CHARTER_MAX_BYTES {
        return Err(ProjectCoordinatorProfileWriteError::InvalidCharter);
    }
    Ok(())
}

fn profile_from_row(row: &sqlx::sqlite::SqliteRow) -> DbResult<ProjectCoordinatorProfile> {
    ProjectCoordinatorProfile::new(
        row.get("charter"),
        row.get("revision"),
        row.get("updated_at_unix_micros"),
    )
    .map_err(|error| crate::DbError::Serialization(error.to_string()))
}

impl Database {
    /// Reads the optional profile owned by one `ProductConversation`.
    ///
    /// # Errors
    ///
    /// Returns a database error when the profile query cannot complete.
    pub async fn get_project_coordinator_profile(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<ProjectCoordinatorProfile>> {
        let row = sqlx::query(
            "SELECT charter, revision, updated_at_unix_micros
             FROM product_conversation_coordinator_profiles
             WHERE product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(profile_from_row).transpose()
    }

    /// Resolves a conversation segment through its stable `ProductConversation` identity.
    ///
    /// # Errors
    ///
    /// Returns a database error when the identity/profile query cannot complete.
    pub async fn get_project_coordinator_profile_for_conversation(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<ProjectCoordinatorProfile>> {
        let row = sqlx::query(
            "SELECT profile.charter, profile.revision, profile.updated_at_unix_micros
             FROM conversations conversation
             JOIN product_conversation_coordinator_profiles profile
               ON profile.product_conversation_id = conversation.product_conversation_id
             WHERE conversation.id = ?1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(profile_from_row).transpose()
    }

    /// Reads the retained revision even when the profile is disabled.
    ///
    /// # Errors
    ///
    /// Returns a database error when the revision query cannot complete.
    pub async fn get_project_coordinator_profile_revision(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT revision FROM product_conversation_coordinator_profile_revisions
             WHERE product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(0))
    }

    /// Creates, revision-fences, or removes the optional profile.
    ///
    /// # Errors
    ///
    /// Returns a domain error for invalid charters, ineligible aggregates, or stale revisions,
    /// and a database error when the transaction cannot complete.
    pub async fn write_project_coordinator_profile(
        &self,
        product_conversation_id: &ProductConversationId,
        charter: Option<&str>,
        expected_revision: i64,
    ) -> Result<ProjectCoordinatorProfileWriteOutcome, ProjectCoordinatorProfileWriteDbError> {
        if let Some(charter) = charter {
            validate_charter(charter)?;
        }
        let now = chrono::Utc::now().timestamp_micros();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let kind: Option<String> =
            sqlx::query_scalar("SELECT kind FROM product_conversations WHERE id = ?1")
                .bind(product_conversation_id.as_str())
                .fetch_optional(&mut *tx)
                .await?;
        if kind.as_deref() != Some("ordinary") {
            return Err(ProjectCoordinatorProfileWriteError::NotOrdinary.into());
        }

        let retained_revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM product_conversation_coordinator_profile_revisions
             WHERE product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(0);
        if retained_revision != expected_revision {
            return Err(ProjectCoordinatorProfileWriteError::RevisionConflict.into());
        }
        sqlx::query(
            "INSERT INTO product_conversation_coordinator_profile_revisions
                 (product_conversation_id, revision)
             VALUES (?1, 1)
             ON CONFLICT(product_conversation_id)
             DO UPDATE SET revision = revision + 1",
        )
        .bind(product_conversation_id.as_str())
        .execute(&mut *tx)
        .await?;
        let new_revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM product_conversation_coordinator_profile_revisions
             WHERE product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_one(&mut *tx)
        .await?;

        let outcome = if let Some(charter) = charter {
            sqlx::query(
                "INSERT INTO product_conversation_coordinator_profiles
                         (product_conversation_id, charter, revision, updated_at_unix_micros)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(product_conversation_id) DO UPDATE SET
                         charter = excluded.charter,
                         revision = excluded.revision,
                         updated_at_unix_micros = excluded.updated_at_unix_micros",
            )
            .bind(product_conversation_id.as_str())
            .bind(charter)
            .bind(new_revision)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            ProjectCoordinatorProfileWriteOutcome::Saved(ProjectCoordinatorProfile::new(
                charter.to_string(),
                new_revision,
                now,
            )?)
        } else {
            sqlx::query(
                "DELETE FROM product_conversation_coordinator_profiles
                 WHERE product_conversation_id = ?1",
            )
            .bind(product_conversation_id.as_str())
            .execute(&mut *tx)
            .await?;
            ProjectCoordinatorProfileWriteOutcome::Disabled
        };

        if tx.commit().await.is_err()
            && !self
                .project_coordinator_write_matches(
                    product_conversation_id,
                    charter,
                    new_revision,
                    now,
                )
                .await
        {
            return Err(ProjectCoordinatorProfileWriteDbError::AmbiguousCommit);
        }
        Ok(outcome)
    }

    async fn project_coordinator_write_matches(
        &self,
        product_conversation_id: &ProductConversationId,
        charter: Option<&str>,
        revision: i64,
        updated_at_unix_micros: i64,
    ) -> bool {
        let persisted_revision: Result<Option<i64>, _> = sqlx::query_scalar(
            "SELECT revision FROM product_conversation_coordinator_profile_revisions
             WHERE product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await;
        if !matches!(persisted_revision, Ok(Some(value)) if value == revision) {
            return false;
        }
        match charter {
            Some(charter) => sqlx::query(
                "SELECT 1 FROM product_conversation_coordinator_profiles
                 WHERE product_conversation_id = ?1 AND charter = ?2
                   AND revision = ?3 AND updated_at_unix_micros = ?4",
            )
            .bind(product_conversation_id.as_str())
            .bind(charter)
            .bind(revision)
            .bind(updated_at_unix_micros)
            .fetch_optional(&self.pool)
            .await
            .is_ok_and(|row| row.is_some()),
            None => self
                .get_project_coordinator_profile(product_conversation_id)
                .await
                .is_ok_and(|profile| profile.is_none()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn product_conversation(db: &Database, id: &str, kind: &str) -> ProductConversationId {
        let product_conversation_id = ProductConversationId::parse(id).expect("valid id");
        let lifecycle = (kind == "ordinary").then_some("open");
        sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES (?1, ?2, ?3)",
        )
        .bind(product_conversation_id.as_str())
        .bind(kind)
        .bind(lifecycle)
        .execute(&db.pool)
        .await
        .expect("insert ProductConversation");
        product_conversation_id
    }

    async fn ordinary(db: &Database, id: &str) -> ProductConversationId {
        product_conversation(db, id, "ordinary").await
    }

    #[tokio::test]
    async fn profile_round_trips_exact_charter_and_rejects_stale_save() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-round-trip").await;
        let charter = "  preserve whitespace\nsecond line  ";
        let created = db
            .write_project_coordinator_profile(&id, Some(charter), 0)
            .await
            .expect("create profile");
        assert!(matches!(
            created,
            ProjectCoordinatorProfileWriteOutcome::Saved(ref profile)
                if profile.revision() == 1
        ));
        assert_eq!(
            db.get_project_coordinator_profile(&id)
                .await
                .expect("read profile")
                .expect("profile")
                .charter(),
            charter
        );

        db.write_project_coordinator_profile(&id, Some("accepted"), 1)
            .await
            .expect("current save");
        let stale = db
            .write_project_coordinator_profile(&id, Some("stale"), 1)
            .await
            .expect_err("stale save must conflict");
        assert!(matches!(
            stale,
            ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::RevisionConflict
            )
        ));
        assert_eq!(
            db.get_project_coordinator_profile(&id)
                .await
                .expect("read profile")
                .expect("profile")
                .charter(),
            "accepted"
        );
    }

    #[tokio::test]
    async fn profile_bounds_and_disable_revision_are_enforced() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-bounds").await;
        for invalid in ["nul\0charter".to_string(), "é".repeat(16_385)] {
            let error = db
                .write_project_coordinator_profile(&id, Some(&invalid), 0)
                .await
                .expect_err("invalid charter");
            assert!(matches!(
                error,
                ProjectCoordinatorProfileWriteDbError::Domain(
                    ProjectCoordinatorProfileWriteError::InvalidCharter
                )
            ));
        }
        db.write_project_coordinator_profile(&id, Some("enabled"), 0)
            .await
            .expect("enable");
        db.write_project_coordinator_profile(&id, None, 1)
            .await
            .expect("disable");
        assert_eq!(
            db.get_project_coordinator_profile(&id)
                .await
                .expect("read profile"),
            None
        );
    }

    #[tokio::test]
    async fn profile_survives_database_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("project-coordinator.db");
        let path = path.to_str().expect("utf-8 path");
        let db = Database::open(path).await.expect("database");
        crate::migrations::run_pending_migrations(&db.pool)
            .await
            .expect("numbered migrations");
        let id = ordinary(&db, "pc-project-coordinator-restart").await;
        db.write_project_coordinator_profile(&id, Some("restart charter"), 0)
            .await
            .expect("profile");
        db.pool.close().await;

        let reopened = Database::open(path).await.expect("reopened database");
        crate::migrations::run_pending_migrations(&reopened.pool)
            .await
            .expect("reopened numbered migrations");
        let profile = reopened
            .get_project_coordinator_profile(&id)
            .await
            .expect("read profile")
            .expect("profile");
        assert_eq!(profile.charter(), "restart charter");
        assert_eq!(profile.revision(), 1);
    }

    #[tokio::test]
    async fn conversation_lookup_resolves_latest_charter_through_product_identity() {
        let db = Database::open_in_memory().await.expect("database");
        db.create_conversation(
            "conversation-segment",
            "conversation-segment",
            "/tmp",
            true,
            None,
            Some("test"),
        )
        .await
        .expect("conversation segment");
        let product_id: String = sqlx::query_scalar(
            "SELECT product_conversation_id FROM conversations WHERE id = 'conversation-segment'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("product identity");
        let id = ProductConversationId::parse(product_id).expect("valid identity");
        db.write_project_coordinator_profile(&id, Some("stale charter"), 0)
            .await
            .expect("profile");
        db.write_project_coordinator_profile(&id, Some("current charter"), 1)
            .await
            .expect("profile edit");

        let profile = db
            .get_project_coordinator_profile_for_conversation("conversation-segment")
            .await
            .expect("lookup")
            .expect("profile");
        assert_eq!(profile.charter(), "current charter");
        assert_eq!(profile.revision(), 2);
    }

    #[tokio::test]
    async fn disable_and_reenable_never_reuse_revision() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-aba").await;
        db.write_project_coordinator_profile(&id, Some("first"), 0)
            .await
            .expect("enable");
        db.write_project_coordinator_profile(&id, None, 1)
            .await
            .expect("disable");
        let stale_disabled = db
            .write_project_coordinator_profile(&id, Some("stale disabled editor"), 0)
            .await
            .expect_err("disabled stale base must conflict");
        assert!(matches!(
            stale_disabled,
            ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::RevisionConflict
            )
        ));
        let reenabled = db
            .write_project_coordinator_profile(&id, Some("second"), 2)
            .await
            .expect("reenable");
        assert!(matches!(
            reenabled,
            ProjectCoordinatorProfileWriteOutcome::Saved(ref profile)
                if profile.revision() == 3
        ));
        let stale = db
            .write_project_coordinator_profile(&id, Some("stale"), 1)
            .await
            .expect_err("old incarnation must conflict");
        assert!(matches!(
            stale,
            ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::RevisionConflict
            )
        ));
    }

    #[tokio::test]
    async fn concurrent_create_accepts_exactly_one_writer() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-concurrent").await;
        let (left, right) = tokio::join!(
            db.write_project_coordinator_profile(&id, Some("left"), 0),
            db.write_project_coordinator_profile(&id, Some("right"), 0),
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let conflict = if left.is_err() { left } else { right }.expect_err("one conflict");
        assert!(matches!(
            conflict,
            ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::RevisionConflict
            )
        ));
        let stored = db
            .get_project_coordinator_profile(&id)
            .await
            .expect("read profile")
            .expect("profile");
        assert_eq!(stored.revision(), 1);
        assert!(stored.charter() == "left" || stored.charter() == "right");
    }

    #[tokio::test]
    async fn profiled_product_conversation_cannot_be_retyped_as_global() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-kind-fence").await;
        db.write_project_coordinator_profile(&id, Some("charter"), 0)
            .await
            .expect("profile");
        let error = sqlx::query(
            "UPDATE product_conversations
             SET kind = 'coordinator', ordinary_lifecycle = NULL
             WHERE id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect_err("kind change must be fenced");
        assert!(error
            .to_string()
            .contains("Project Coordinator profile must remain ordinary"));
    }

    #[tokio::test]
    async fn global_coordinator_cannot_acquire_profile() {
        let db = Database::open_in_memory().await.expect("database");
        let id = product_conversation(&db, "pc-global-coordinator", "coordinator").await;
        let error = db
            .write_project_coordinator_profile(&id, Some("not allowed"), 0)
            .await
            .expect_err("global profile must be rejected");
        assert!(matches!(
            error,
            ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::NotOrdinary
            )
        ));
    }
}
