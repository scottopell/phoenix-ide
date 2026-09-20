use phoenix_core::domain::product_conversation::{
    ProductConversationId, ProjectCoordinatorProfile, ProjectCoordinatorProfileWriteError,
    PROJECT_COORDINATOR_CHARTER_MAX_BYTES,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{Database, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectCoordinatorProfileWriteOutcome {
    Saved(ProjectCoordinatorProfile),
    Disabled { revision: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCoordinatorProfileSettings {
    pub profile: Option<ProjectCoordinatorProfile>,
    pub revision: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectCoordinatorProfileWriteDbError {
    #[error(transparent)]
    Domain(#[from] ProjectCoordinatorProfileWriteError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Project Coordinator profile commit outcome is unclassifiable")]
    AmbiguousCommit,
    #[error("Project Coordinator profile commit did not apply")]
    NotCommitted,
}

fn validate_charter(charter: &str) -> Result<(), ProjectCoordinatorProfileWriteError> {
    if charter.as_bytes().contains(&0) || charter.len() > PROJECT_COORDINATOR_CHARTER_MAX_BYTES {
        return Err(ProjectCoordinatorProfileWriteError::InvalidCharter);
    }
    Ok(())
}

async fn active_profile_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    product_conversation_id: &ProductConversationId,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM product_conversation_coordinator_profiles
             WHERE product_conversation_id = ?1
         )",
    )
    .bind(product_conversation_id.as_str())
    .fetch_one(&mut **tx)
    .await
}

async fn admit_project_coordinator_profile_insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    product_conversation_id: &ProductConversationId,
    write_token: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO product_conversation_coordinator_profile_insert_admissions
             (product_conversation_id, write_token)
         VALUES (?1, ?2)
         ON CONFLICT(product_conversation_id)
         DO UPDATE SET write_token = excluded.write_token",
    )
    .bind(product_conversation_id.as_str())
    .bind(write_token)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn retained_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    product_conversation_id: &ProductConversationId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT revision FROM product_conversation_coordinator_profile_revisions
         WHERE product_conversation_id = ?1",
    )
    .bind(product_conversation_id.as_str())
    .fetch_one(&mut **tx)
    .await
}

async fn advance_project_coordinator_profile_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    product_conversation_id: &ProductConversationId,
    write_token: &str,
    active_exists: bool,
) -> Result<i64, sqlx::Error> {
    if active_exists {
        sqlx::query(
            "DELETE FROM product_conversation_coordinator_profiles
             WHERE product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE product_conversation_coordinator_profile_revisions
             SET last_write_token = ?2
             WHERE product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .bind(write_token)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO product_conversation_coordinator_profile_revisions
                 (product_conversation_id, revision, last_write_token)
             VALUES (?1, 1, ?2)
             ON CONFLICT(product_conversation_id)
             DO UPDATE SET revision = revision + 1,
                           last_write_token = excluded.last_write_token",
        )
        .bind(product_conversation_id.as_str())
        .bind(write_token)
        .execute(&mut **tx)
        .await?;
    }
    retained_revision(tx, product_conversation_id).await
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
            "SELECT profile.charter, fence.revision, profile.updated_at_unix_micros
             FROM product_conversation_coordinator_profiles profile
             JOIN product_conversation_coordinator_profile_revisions fence
               ON fence.product_conversation_id = profile.product_conversation_id
             WHERE profile.product_conversation_id = ?1",
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
            "SELECT profile.charter, fence.revision, profile.updated_at_unix_micros
             FROM conversations conversation
             JOIN product_conversation_coordinator_profiles profile
               ON profile.product_conversation_id = conversation.product_conversation_id
             JOIN product_conversation_coordinator_profile_revisions fence
               ON fence.product_conversation_id = profile.product_conversation_id
             WHERE conversation.id = ?1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(profile_from_row).transpose()
    }

    /// Reads the active profile and retained revision from one `SQLite` snapshot.
    ///
    /// # Errors
    ///
    /// Returns a database error when the settings query cannot complete or persisted profile
    /// fields violate domain invariants.
    pub async fn get_project_coordinator_profile_settings(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<ProjectCoordinatorProfileSettings> {
        let row = sqlx::query(
            "SELECT profile.charter,
                    profile.updated_at_unix_micros,
                    COALESCE(fence.revision, 0) AS retained_revision
             FROM product_conversations conversation
             LEFT JOIN product_conversation_coordinator_profiles profile
               ON profile.product_conversation_id = conversation.id
             LEFT JOIN product_conversation_coordinator_profile_revisions fence
               ON fence.product_conversation_id = conversation.id
             WHERE conversation.id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_one(&self.pool)
        .await?;
        let revision = row.get("retained_revision");
        let charter: Option<String> = row.try_get("charter")?;
        let profile = charter
            .map(|charter| {
                ProjectCoordinatorProfile::new(charter, revision, row.get("updated_at_unix_micros"))
                    .map_err(|error| crate::DbError::Serialization(error.to_string()))
            })
            .transpose()?;
        Ok(ProjectCoordinatorProfileSettings { profile, revision })
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
        let write_token = Uuid::new_v4().to_string();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure_project_coordinator_profile_writable(&mut tx, product_conversation_id).await?;

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
        let active_exists = active_profile_exists(&mut tx, product_conversation_id).await?;

        let new_revision;
        let outcome = if let Some(charter) = charter {
            new_revision = advance_project_coordinator_profile_revision(
                &mut tx,
                product_conversation_id,
                &write_token,
                active_exists,
            )
            .await?;
            admit_project_coordinator_profile_insert(
                &mut tx,
                product_conversation_id,
                &write_token,
            )
            .await?;
            sqlx::query(
                "INSERT INTO product_conversation_coordinator_profiles
                         (product_conversation_id, charter, updated_at_unix_micros)
                     VALUES (?1, ?2, ?3)",
            )
            .bind(product_conversation_id.as_str())
            .bind(charter)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            ProjectCoordinatorProfileWriteOutcome::Saved(ProjectCoordinatorProfile::new(
                charter.to_string(),
                new_revision,
                now,
            )?)
        } else {
            new_revision = advance_project_coordinator_profile_revision(
                &mut tx,
                product_conversation_id,
                &write_token,
                active_exists,
            )
            .await?;
            ProjectCoordinatorProfileWriteOutcome::Disabled {
                revision: new_revision,
            }
        };

        if tx.commit().await.is_err() {
            return match self
                .classify_project_coordinator_write(
                    product_conversation_id,
                    charter,
                    expected_revision,
                    new_revision,
                    now,
                    &write_token,
                )
                .await
            {
                Some(true) => Ok(outcome),
                Some(false) => Err(ProjectCoordinatorProfileWriteDbError::NotCommitted),
                None => Err(ProjectCoordinatorProfileWriteDbError::AmbiguousCommit),
            };
        }
        Ok(outcome)
    }

    async fn classify_project_coordinator_write(
        &self,
        product_conversation_id: &ProductConversationId,
        charter: Option<&str>,
        expected_revision: i64,
        revision: i64,
        updated_at_unix_micros: i64,
        write_token: &str,
    ) -> Option<bool> {
        let classified: Result<Option<(i64, String, Option<i64>)>, _> = sqlx::query_as(
            "SELECT fence.revision,
                    fence.last_write_token,
                    CASE
                      WHEN profile.product_conversation_id IS NOT NULL
                       AND profile.charter IS ?2
                       AND profile.updated_at_unix_micros = ?3
                      THEN 1 ELSE 0
                    END AS intended_match
             FROM product_conversation_coordinator_profile_revisions fence
             LEFT JOIN product_conversation_coordinator_profiles profile
               ON profile.product_conversation_id = fence.product_conversation_id
             WHERE fence.product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .bind(charter)
        .bind(updated_at_unix_micros)
        .fetch_optional(&self.pool)
        .await;
        let Some((persisted_revision, persisted_write_token, intended_match)) = classified.ok()?
        else {
            return (expected_revision == 0).then_some(false);
        };
        if persisted_revision == expected_revision {
            return Some(false);
        }
        if persisted_revision != revision {
            return None;
        }
        if persisted_write_token != write_token {
            return None;
        }
        match charter {
            Some(_) => intended_match.map(|value| value == 1),
            None => Some(intended_match == Some(0)),
        }
    }
}

async fn ensure_project_coordinator_profile_writable(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    product_conversation_id: &ProductConversationId,
) -> Result<(), ProjectCoordinatorProfileWriteDbError> {
    let aggregate: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT kind, ordinary_lifecycle FROM product_conversations WHERE id = ?1")
            .bind(product_conversation_id.as_str())
            .fetch_optional(&mut **tx)
            .await?;
    let Some((kind, ordinary_lifecycle)) = aggregate else {
        return Err(ProjectCoordinatorProfileWriteError::NotOrdinary.into());
    };
    if kind != "ordinary" {
        return Err(ProjectCoordinatorProfileWriteError::NotOrdinary.into());
    }
    if ordinary_lifecycle.as_deref() != Some("open") {
        return Err(ProjectCoordinatorProfileWriteError::NotOpen.into());
    }
    Ok(())
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
    async fn active_profile_stores_no_parallel_revision() {
        let db = Database::open_in_memory().await.expect("database");
        let columns = sqlx::query(
            "SELECT name FROM pragma_table_info('product_conversation_coordinator_profiles')",
        )
        .fetch_all(&db.pool)
        .await
        .expect("columns");
        assert!(!columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "revision"));
    }

    #[tokio::test]
    async fn active_profile_requires_retained_revision_row() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-positive-revision").await;
        let insert_error = sqlx::query(
            "INSERT INTO product_conversation_coordinator_profiles
                 (product_conversation_id, charter, updated_at_unix_micros)
             VALUES (?1, 'invalid active charter', 1)",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect_err("active profile must require a retained revision row");
        assert!(insert_error
            .to_string()
            .contains("Project Coordinator active profile requires positive revision"));

        sqlx::query(
            "INSERT INTO product_conversation_coordinator_profile_revisions
                 (product_conversation_id, revision, last_write_token)
             VALUES (?1, 0, 'seed')",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect("zero retained revision");
        sqlx::query(
            "INSERT INTO product_conversation_coordinator_profiles
                 (product_conversation_id, charter, updated_at_unix_micros)
             VALUES (?1, 'valid active charter', 2)",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect("active profile");
        assert_eq!(
            db.get_project_coordinator_profile_revision(&id)
                .await
                .unwrap(),
            1
        );
        let update_error = sqlx::query(
            "UPDATE product_conversation_coordinator_profile_revisions
             SET revision = 0
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect_err("active profile must keep a positive retained revision");
        assert!(update_error
            .to_string()
            .contains("Project Coordinator profile revision cannot roll back"));
    }

    #[tokio::test]
    async fn direct_active_profile_delete_advances_retained_revision() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-direct-delete").await;
        db.write_project_coordinator_profile(&id, Some("first"), 0)
            .await
            .expect("enable");
        sqlx::query(
            "DELETE FROM product_conversation_coordinator_profiles
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect("direct delete");
        let retained_revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM product_conversation_coordinator_profile_revisions
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .fetch_one(&db.pool)
        .await
        .expect("retained revision");
        assert_eq!(retained_revision, 2);
        assert!(matches!(
            db.write_project_coordinator_profile(&id, Some("stale re-enable"), 1)
                .await,
            Err(ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::RevisionConflict
            ))
        ));
    }

    #[tokio::test]
    async fn direct_active_profile_insert_advances_retained_revision() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-direct-insert").await;
        db.write_project_coordinator_profile(&id, Some("first"), 0)
            .await
            .expect("enable");
        db.write_project_coordinator_profile(&id, None, 1)
            .await
            .expect("disable");
        sqlx::query(
            "INSERT INTO product_conversation_coordinator_profiles
                 (product_conversation_id, charter, updated_at_unix_micros)
             VALUES (?1, 'direct active charter', 2)",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect("direct insert");
        let retained_revision = db
            .get_project_coordinator_profile_revision(&id)
            .await
            .expect("retained revision");
        assert_eq!(retained_revision, 3);
        assert!(matches!(
            db.write_project_coordinator_profile(&id, Some("stale overwrite"), 2)
                .await,
            Err(ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::RevisionConflict
            ))
        ));
    }

    #[tokio::test]
    async fn direct_retained_revision_delete_is_rejected_while_owner_exists() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-direct-revision-delete").await;
        db.write_project_coordinator_profile(&id, Some("first"), 0)
            .await
            .expect("enable");
        let delete_error = sqlx::query(
            "DELETE FROM product_conversation_coordinator_profile_revisions
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect_err("active retained revision delete must be rejected");
        assert!(delete_error
            .to_string()
            .contains("Project Coordinator retained revision"));
        db.write_project_coordinator_profile(&id, None, 1)
            .await
            .expect("disable");
        let disabled_delete_error = sqlx::query(
            "DELETE FROM product_conversation_coordinator_profile_revisions
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect_err("disabled retained revision delete must be rejected");
        assert!(disabled_delete_error
            .to_string()
            .contains("Project Coordinator retained revision"));
    }

    #[tokio::test]
    async fn direct_retained_revision_owner_update_is_rejected() {
        let db = Database::open_in_memory().await.expect("database");
        let source = ordinary(&db, "pc-project-coordinator-revision-owner-source").await;
        let destination = ordinary(&db, "pc-project-coordinator-revision-owner-destination").await;
        db.write_project_coordinator_profile(&source, Some("first"), 0)
            .await
            .expect("enable");
        db.write_project_coordinator_profile(&source, None, 1)
            .await
            .expect("disable");
        let update_error = sqlx::query(
            "UPDATE product_conversation_coordinator_profile_revisions
             SET product_conversation_id = ?2
             WHERE product_conversation_id = ?1",
        )
        .bind(source.as_str())
        .bind(destination.as_str())
        .execute(&db.pool)
        .await
        .expect_err("retained revision owner move must be rejected");
        assert!(update_error
            .to_string()
            .contains("Project Coordinator retained revision owner is immutable"));
    }

    #[tokio::test]
    async fn active_profile_owner_cannot_move_without_revision_fence() {
        let db = Database::open_in_memory().await.expect("database");
        let source = ordinary(&db, "pc-project-coordinator-owner-source").await;
        let destination = ordinary(&db, "pc-project-coordinator-owner-destination").await;
        db.write_project_coordinator_profile(&source, Some("first"), 0)
            .await
            .expect("enable source");
        sqlx::query(
            "INSERT INTO product_conversation_coordinator_profile_revisions
                 (product_conversation_id, revision, last_write_token)
             VALUES (?1, 1, 'destination')",
        )
        .bind(destination.as_str())
        .execute(&db.pool)
        .await
        .expect("destination retained revision");
        let update_error = sqlx::query(
            "UPDATE product_conversation_coordinator_profiles
             SET product_conversation_id = ?2
             WHERE product_conversation_id = ?1",
        )
        .bind(source.as_str())
        .bind(destination.as_str())
        .execute(&db.pool)
        .await
        .expect_err("active profile owner move must be rejected");
        assert!(update_error
            .to_string()
            .contains("Project Coordinator active profile owner is immutable"));
    }

    #[tokio::test]
    async fn active_profile_content_cannot_update_without_revision_fence() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-direct-update").await;
        db.write_project_coordinator_profile(&id, Some("first"), 0)
            .await
            .expect("enable");
        let update_error = sqlx::query(
            "UPDATE product_conversation_coordinator_profiles
             SET charter = 'direct edit'
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect_err("direct active content update must be rejected");
        assert!(update_error.to_string().contains(
            "Project Coordinator active profile content is replaced through the revision fence"
        ));
    }

    #[tokio::test]
    async fn retained_revision_cannot_roll_back() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-rollback").await;
        db.write_project_coordinator_profile(&id, Some("first"), 0)
            .await
            .expect("enable");
        db.write_project_coordinator_profile(&id, None, 1)
            .await
            .expect("disable");
        let rollback_error = sqlx::query(
            "UPDATE product_conversation_coordinator_profile_revisions
             SET revision = 1
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect_err("retained revision must not roll back");
        assert!(rollback_error
            .to_string()
            .contains("Project Coordinator profile revision cannot roll back"));
    }

    #[tokio::test]
    async fn moving_active_profile_is_rejected_before_destination_revision_check() {
        let db = Database::open_in_memory().await.expect("database");
        let source = ordinary(&db, "pc-project-coordinator-move-source").await;
        let destination = ordinary(&db, "pc-project-coordinator-move-destination").await;
        db.write_project_coordinator_profile(&source, Some("source charter"), 0)
            .await
            .expect("source profile");
        sqlx::query(
            "INSERT INTO product_conversation_coordinator_profile_revisions
                 (product_conversation_id, revision, last_write_token)
             VALUES (?1, 0, 'destination-seed')",
        )
        .bind(destination.as_str())
        .execute(&db.pool)
        .await
        .expect("destination zero retained revision");

        let move_error = sqlx::query(
            "UPDATE product_conversation_coordinator_profiles
             SET product_conversation_id = ?1
             WHERE product_conversation_id = ?2",
        )
        .bind(destination.as_str())
        .bind(source.as_str())
        .execute(&db.pool)
        .await
        .expect_err("active profile owner move must be rejected");
        assert!(move_error.to_string().contains("Project Coordinator"));
    }

    #[tokio::test]
    async fn disable_commit_classification_requires_write_token_identity() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-disable-token").await;
        db.write_project_coordinator_profile(&id, Some("enabled"), 0)
            .await
            .expect("enable");
        sqlx::query(
            "DELETE FROM product_conversation_coordinator_profiles
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect("simulate disabled profile");
        sqlx::query(
            "UPDATE product_conversation_coordinator_profile_revisions
             SET revision = 2, last_write_token = 'other-disable'
             WHERE product_conversation_id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect("simulate concurrent retained disable revision");

        assert_eq!(
            db.classify_project_coordinator_write(&id, None, 1, 2, 1, "this-disable")
                .await,
            None
        );
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
        let disabled = db
            .get_project_coordinator_profile_settings(&id)
            .await
            .expect("disabled settings");
        assert_eq!(disabled.profile, None);
        assert_eq!(disabled.revision, 2);
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
    async fn history_product_conversation_cannot_mutate_profile() {
        let db = Database::open_in_memory().await.expect("database");
        let id = ordinary(&db, "pc-project-coordinator-history").await;
        db.write_project_coordinator_profile(&id, Some("open charter"), 0)
            .await
            .expect("open profile");
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1",
        )
        .bind(id.as_str())
        .execute(&db.pool)
        .await
        .expect("close to History");

        let enable_error = db
            .write_project_coordinator_profile(&id, Some("history edit"), 1)
            .await
            .expect_err("History profile edit must be rejected");
        assert!(matches!(
            enable_error,
            ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::NotOpen
            )
        ));
        let disable_error = db
            .write_project_coordinator_profile(&id, None, 1)
            .await
            .expect_err("History profile disable must be rejected");
        assert!(matches!(
            disable_error,
            ProjectCoordinatorProfileWriteDbError::Domain(
                ProjectCoordinatorProfileWriteError::NotOpen
            )
        ));
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
