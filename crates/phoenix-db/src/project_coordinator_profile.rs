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
}

fn validate_charter(charter: &str) -> Result<(), ProjectCoordinatorProfileWriteError> {
    if charter.as_bytes().contains(&0) || charter.len() > PROJECT_COORDINATOR_CHARTER_MAX_BYTES {
        return Err(ProjectCoordinatorProfileWriteError::InvalidCharter);
    }
    Ok(())
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
        Ok(row.map(|row| ProjectCoordinatorProfile {
            charter: row.get("charter"),
            revision: row.get("revision"),
            updated_at_unix_micros: row.get("updated_at_unix_micros"),
        }))
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
        Ok(row.map(|row| ProjectCoordinatorProfile {
            charter: row.get("charter"),
            revision: row.get("revision"),
            updated_at_unix_micros: row.get("updated_at_unix_micros"),
        }))
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
        expected_revision: Option<i64>,
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

        let rows_affected = match (charter, expected_revision) {
            (Some(charter), None) => {
                sqlx::query(
                    "INSERT INTO product_conversation_coordinator_profiles
                     (product_conversation_id, charter, revision, updated_at_unix_micros)
                 VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(product_conversation_id) DO NOTHING",
                )
                .bind(product_conversation_id.as_str())
                .bind(charter)
                .bind(now)
                .execute(&mut *tx)
                .await
            }
            (Some(charter), Some(revision)) => {
                sqlx::query(
                    "UPDATE product_conversation_coordinator_profiles
                 SET charter = ?2, revision = revision + 1, updated_at_unix_micros = ?3
                 WHERE product_conversation_id = ?1 AND revision = ?4",
                )
                .bind(product_conversation_id.as_str())
                .bind(charter)
                .bind(now)
                .bind(revision)
                .execute(&mut *tx)
                .await
            }
            (None, Some(revision)) => {
                sqlx::query(
                    "DELETE FROM product_conversation_coordinator_profiles
                 WHERE product_conversation_id = ?1 AND revision = ?2",
                )
                .bind(product_conversation_id.as_str())
                .bind(revision)
                .execute(&mut *tx)
                .await
            }
            (None, None) => {
                return Err(ProjectCoordinatorProfileWriteError::RevisionConflict.into())
            }
        }?
        .rows_affected();

        if rows_affected != 1 {
            return Err(ProjectCoordinatorProfileWriteError::RevisionConflict.into());
        }
        tx.commit().await?;

        match charter {
            Some(charter) => Ok(ProjectCoordinatorProfileWriteOutcome::Saved(
                ProjectCoordinatorProfile {
                    charter: charter.to_string(),
                    revision: expected_revision.map_or(1, |revision| revision + 1),
                    updated_at_unix_micros: now,
                },
            )),
            None => Ok(ProjectCoordinatorProfileWriteOutcome::Disabled),
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
            .write_project_coordinator_profile(&id, Some(charter), None)
            .await
            .expect("create profile");
        assert!(matches!(
            created,
            ProjectCoordinatorProfileWriteOutcome::Saved(ProjectCoordinatorProfile {
                revision: 1,
                ..
            })
        ));
        assert_eq!(
            db.get_project_coordinator_profile(&id)
                .await
                .expect("read profile")
                .expect("profile")
                .charter,
            charter
        );

        db.write_project_coordinator_profile(&id, Some("accepted"), Some(1))
            .await
            .expect("current save");
        let stale = db
            .write_project_coordinator_profile(&id, Some("stale"), Some(1))
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
                .charter,
            "accepted"
        );
    }

    #[tokio::test]
    async fn profile_bounds_and_disable_revision_are_enforced() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-bounds").await;
        for invalid in ["nul\0charter".to_string(), "é".repeat(16_385)] {
            let error = db
                .write_project_coordinator_profile(&id, Some(&invalid), None)
                .await
                .expect_err("invalid charter");
            assert!(matches!(
                error,
                ProjectCoordinatorProfileWriteDbError::Domain(
                    ProjectCoordinatorProfileWriteError::InvalidCharter
                )
            ));
        }
        db.write_project_coordinator_profile(&id, Some("enabled"), None)
            .await
            .expect("enable");
        db.write_project_coordinator_profile(&id, None, Some(1))
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
        db.write_project_coordinator_profile(&id, Some("restart charter"), None)
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
        assert_eq!(profile.charter, "restart charter");
        assert_eq!(profile.revision, 1);
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
        db.write_project_coordinator_profile(&id, Some("stale charter"), None)
            .await
            .expect("profile");
        db.write_project_coordinator_profile(&id, Some("current charter"), Some(1))
            .await
            .expect("profile edit");

        let profile = db
            .get_project_coordinator_profile_for_conversation("conversation-segment")
            .await
            .expect("lookup")
            .expect("profile");
        assert_eq!(profile.charter, "current charter");
        assert_eq!(profile.revision, 2);
    }

    #[tokio::test]
    async fn concurrent_create_accepts_exactly_one_writer() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-concurrent").await;
        let (left, right) = tokio::join!(
            db.write_project_coordinator_profile(&id, Some("left"), None),
            db.write_project_coordinator_profile(&id, Some("right"), None),
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
        assert_eq!(stored.revision, 1);
        assert!(stored.charter == "left" || stored.charter == "right");
    }

    #[tokio::test]
    async fn profiled_product_conversation_cannot_be_retyped_as_global() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-kind-fence").await;
        db.write_project_coordinator_profile(&id, Some("charter"), None)
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
            .write_project_coordinator_profile(&id, Some("not allowed"), None)
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
